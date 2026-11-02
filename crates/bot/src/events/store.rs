//! The append-only product event log on foundation Postgres.
//!
//! [`EventLog`] is a **sink, not a service**: [`record`](EventLog::record) takes
//! an event and returns nothing, swallowing and logging every failure, so the
//! user-visible paths that emit can never branch on — or be delayed by — the fact
//! that we happen to be keeping records. That is the same contract
//! [`crate::telemetry::Telemetry`] holds for the Langfuse plane, and it is what
//! makes the degradation matrix true: no database, no events, bot unaffected.
//!
//! Storage follows the conventions already set by [`crate::store`] and
//! [`crate::backup::store`]: snowflakes as `TEXT` (they overflow `BIGINT`'s sign
//! bit), `TIMESTAMPTZ DEFAULT now()`, and an idempotent schema applied at
//! startup rather than a migration tool.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use kube::Client;
use sqlx::postgres::PgPool;
use tracing::{error, info, warn};

use super::catalog::{EventKind, ProductEvent};
use super::resources::ResourceSample;
use super::sampler::OccupancySample;
use crate::config::DbConfig;
use crate::domain::{GameId, GuildId, InstanceName};

/// Postgres pool size for the event log. Two is ample: writes are one small
/// insert per product action at friends scale, never a hot path.
const EVENT_LOG_MAX_CONNECTIONS: u32 = 2;

/// Idempotent schema for the tables of the product plane.
///
/// One wide `events` table rather than a table per action: the catalog is closed
/// and every row answers the same question ("who did what, where, and how did it
/// end"), so the derivation layer's queries are `WHERE kind = ...` rather than a
/// union across a dozen shapes. `guild_id`, `game`, and `instance` are nullable
/// because not every kind has all three.
///
/// `known_guilds` exists so `guild_joined` survives a gateway reconnect, which
/// replays `GuildCreate` for every guild the bot is already in.
///
/// `occupancy_samples` is the occupancy time series: one row per running
/// instance per sampler tick, which the derivation layer integrates into playtime
/// and differences into crash frequency. `players` is nullable and null means
/// *unknown* — the game has no working occupancy probe, or its console wasn't
/// reachable this tick — never a definite zero. Both counters are `BIGINT` so the
/// supervisor's `u32`s bind without a fallible narrowing conversion.
///
/// `resource_samples` is the cost time series: one row per *existing* instance
/// per tick — running, paused, or shut down — holding raw quantities only
/// (millicores, bytes), never money. Rates live in the query layer
/// (`scripts/queries/`), so a rate revision reprices history instead of being
/// baked into rows. Every quantity column is nullable and null means that
/// reading wasn't available this tick (no pod, or metrics-server had nothing) —
/// never zero; see [`ResourceSample`] for the exact per-state row shapes.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS events (\
    id BIGSERIAL PRIMARY KEY, \
    kind TEXT NOT NULL, \
    guild_id TEXT, \
    game TEXT, \
    instance TEXT, \
    actor TEXT NOT NULL, \
    surface TEXT NOT NULL, \
    outcome TEXT NOT NULL, \
    details JSONB NOT NULL DEFAULT '{}'::jsonb, \
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS events_created_at_idx ON events (created_at); \
    CREATE INDEX IF NOT EXISTS events_guild_kind_idx ON events (guild_id, kind, created_at); \
    CREATE TABLE IF NOT EXISTS known_guilds (\
    guild_id TEXT PRIMARY KEY, \
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE TABLE IF NOT EXISTS occupancy_samples (\
    id BIGSERIAL PRIMARY KEY, \
    instance TEXT NOT NULL, \
    game TEXT, \
    guild_id TEXT, \
    players BIGINT, \
    restarts BIGINT NOT NULL, \
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS occupancy_samples_sampled_at_idx ON occupancy_samples (sampled_at); \
    CREATE INDEX IF NOT EXISTS occupancy_samples_instance_idx ON occupancy_samples (instance, sampled_at); \
    CREATE TABLE IF NOT EXISTS resource_samples (\
    id BIGSERIAL PRIMARY KEY, \
    instance TEXT NOT NULL, \
    game TEXT, \
    guild_id TEXT, \
    cpu_millicores BIGINT, \
    mem_bytes BIGINT, \
    cpu_request_millicores BIGINT, \
    mem_request_bytes BIGINT, \
    cpu_limit_millicores BIGINT, \
    mem_limit_bytes BIGINT, \
    pvc_bytes BIGINT, \
    sampled_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS resource_samples_sampled_at_idx ON resource_samples (sampled_at); \
    CREATE INDEX IF NOT EXISTS resource_samples_instance_idx ON resource_samples (instance, sampled_at)";

/// `details` is bound as text and cast, because sqlx is pinned without its `json`
/// feature — the cast costs nothing and keeps the dependency surface as it is.
const INSERT_EVENT: &str = "INSERT INTO events \
    (kind, guild_id, game, instance, actor, surface, outcome, details) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)";

/// Newest-first page of the log, with every filter optional. A `NULL` bind makes
/// its clause vacuous, so one prepared statement serves every combination the
/// panel offers rather than assembling SQL per request.
const RECENT_EVENTS: &str = "SELECT id, kind, guild_id, game, instance, actor, surface, outcome, \
    details::text, created_at::text FROM events \
    WHERE ($1::text IS NULL OR kind = $1) \
    AND ($2::text IS NULL OR guild_id = $2) \
    AND ($3::text IS NULL OR instance = $3) \
    AND ($4::text IS NULL OR surface = $4) \
    AND ($5::bigint IS NULL OR id < $5) \
    ORDER BY id DESC LIMIT $6";

/// Which slice of the event log to show. Every field unset is "everything,
/// newest first".
#[derive(Clone, Debug, Default)]
pub(crate) struct EventFilter {
    pub(crate) kind: Option<String>,
    pub(crate) guild: Option<String>,
    pub(crate) instance: Option<String>,
    pub(crate) surface: Option<String>,
    /// Page backwards from this id, exclusive. Keyset rather than offset, so a
    /// page stays put as new events land at the head.
    pub(crate) before_id: Option<i64>,
}

/// One event as the panel renders it. Timestamps and `details` arrive as text
/// because `sqlx` here carries neither a date nor a json feature.
type EventRowColumns = (
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
);

/// One row of the event log, named.
#[derive(Clone, Debug)]
pub(crate) struct EventRow {
    pub(crate) id: i64,
    pub(crate) kind: String,
    pub(crate) guild: Option<String>,
    pub(crate) game: Option<String>,
    pub(crate) instance: Option<String>,
    pub(crate) actor: String,
    pub(crate) surface: String,
    pub(crate) outcome: String,
    pub(crate) details: String,
    pub(crate) created_at: String,
}

impl From<EventRowColumns> for EventRow {
    fn from(columns: EventRowColumns) -> Self {
        let (id, kind, guild, game, instance, actor, surface, outcome, details, created_at) =
            columns;
        Self {
            id,
            kind,
            guild,
            game,
            instance,
            actor,
            surface,
            outcome,
            details,
            created_at,
        }
    }
}

/// Claim a guild as known. `RETURNING` fires only on a real insert, so the row
/// count *is* the answer to "is this a genuine join?" — no read-then-write race.
const CLAIM_GUILD: &str =
    "INSERT INTO known_guilds (guild_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING guild_id";

const SEED_GUILDS: &str =
    "INSERT INTO known_guilds (guild_id) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING";

/// A whole sampler tick in one statement, via parallel arrays. Batching isn't
/// about throughput at this scale — it's that a database outage then costs one
/// error log per tick instead of one per server per tick, which is the
/// difference between a signal and a flood.
const INSERT_SAMPLES: &str = "INSERT INTO occupancy_samples \
    (instance, game, guild_id, players, restarts) \
    SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::bigint[], $5::bigint[])";

/// One statement per resource tick, for the same reason as [`INSERT_SAMPLES`] —
/// and the shared `sampled_at` default it produces is what lets queries group a
/// tick's rows into one fleet-wide reading.
const INSERT_RESOURCE_SAMPLES: &str = "INSERT INTO resource_samples \
    (instance, game, guild_id, cpu_millicores, mem_bytes, cpu_request_millicores, \
    mem_request_bytes, cpu_limit_millicores, mem_limit_bytes, pvc_bytes) \
    SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::bigint[], $5::bigint[], \
    $6::bigint[], $7::bigint[], $8::bigint[], $9::bigint[], $10::bigint[])";

/// The most recent event of each kind for one instance. `DISTINCT ON (kind)`
/// collapses the history to one row per kind, which is what a "when did this last
/// happen" question wants — the alternative is pulling the whole instance's
/// history back and folding it in Rust.
const LATEST_PER_KIND: &str = "SELECT DISTINCT ON (kind) kind, actor, created_at::text \
    FROM events WHERE instance = $1 AND kind = ANY($2) \
    ORDER BY kind, created_at DESC";

/// When something last happened to an instance, and who did it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LatestEvent {
    pub(crate) kind: String,
    pub(crate) actor: String,
    /// RFC-3339-ish timestamp text, for display only — same treatment as
    /// [`crate::backup::ArchiveRecord::created_at`], which avoids a chrono/time
    /// sqlx feature for a value nothing computes on.
    pub(crate) created_at: String,
}

/// The product event log, or a disabled no-op when persistence isn't configured.
pub(crate) struct EventLog {
    pool: Option<PgPool>,
    /// Whether the known-guilds set has been reconciled against the guilds this
    /// process is actually in. Until it has, a `GuildCreate` can't tell a genuine
    /// join from the gateway replaying the guilds we were already in, so joins
    /// are claimed silently — see [`EventLog::claim_guild`].
    seeded: AtomicBool,
}

impl EventLog {
    /// Connect (if configured) and apply the schema. Never fails: any problem is
    /// logged and leaves the log disabled so the rest of the bot keeps working.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            // `HomeChannels::connect` already reported the absent password; this
            // one names what else goes dark because of it.
            warn!("DB_PASSWORD not set; product event log disabled (nothing else changes)");
            return Self::disabled();
        };
        match Self::open(config).await {
            Ok(pool) => {
                info!("connected to postgres for the product event log");
                Self {
                    pool: Some(pool),
                    seeded: AtomicBool::new(false),
                }
            }
            Err(err) => {
                error!(error = ?err, "postgres unavailable; product event log disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            pool: None,
            seeded: AtomicBool::new(false),
        }
    }

    async fn open(config: &DbConfig) -> Result<PgPool> {
        let pool = crate::store::connect_pool(config, EVENT_LOG_MAX_CONNECTIONS).await?;
        // `raw_sql` runs via the simple query protocol so the multi-statement
        // schema executes as one batch; `query` would prepare it and Postgres
        // rejects multiple commands in a prepared statement.
        sqlx::raw_sql(SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply product event log schema")?;
        Ok(pool)
    }

    /// Whether events are being persisted.
    pub(crate) fn enabled(&self) -> bool {
        self.pool.is_some()
    }

    /// Append one event. Failure is logged, never returned: an emission site is
    /// on a user-visible path and has nothing useful to do with the error.
    pub(crate) async fn record(&self, event: ProductEvent) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };
        let details = match serde_json::to_string(&event.details) {
            Ok(details) => details,
            Err(err) => {
                error!(error = ?err, kind = event.kind.as_str(), "failed to serialize event details");
                return;
            }
        };
        let result = sqlx::query(INSERT_EVENT)
            .bind(event.kind.as_str())
            .bind(event.guild.as_ref().map(GuildId::as_str))
            .bind(event.game.as_ref().map(GameId::as_str))
            .bind(event.instance.as_ref().map(InstanceName::as_str))
            .bind(event.actor.label())
            .bind(event.surface.as_str())
            .bind(event.outcome)
            .bind(details)
            .execute(pool)
            .await;
        if let Err(err) = result {
            error!(
                error = ?err,
                kind = event.kind.as_str(),
                outcome = event.outcome,
                "failed to record product event"
            );
        }
    }

    /// The most recent events, newest first, narrowed by whichever filters are
    /// set. Backs the admin panel's browser.
    ///
    /// Returns empty when the log is disabled or the query fails — the panel
    /// shows an empty table rather than an error page, and the failure is logged.
    /// `before_id` pages backwards by id rather than by offset, so a page can't
    /// shift under the reader as new events land.
    pub(crate) async fn recent(&self, filter: &EventFilter, limit: i64) -> Vec<EventRow> {
        let Some(pool) = self.pool.as_ref() else {
            return Vec::new();
        };
        // Every filter is bound rather than interpolated, and a `NULL` bind
        // disables its clause — one prepared statement covers every combination.
        match sqlx::query_as::<_, EventRowColumns>(RECENT_EVENTS)
            .bind(filter.kind.as_deref())
            .bind(filter.guild.as_deref())
            .bind(filter.instance.as_deref())
            .bind(filter.surface.as_deref())
            .bind(filter.before_id)
            .bind(limit)
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows.into_iter().map(EventRow::from).collect(),
            Err(err) => {
                error!(error = ?err, "failed to read recent events");
                Vec::new()
            }
        }
    }

    /// The distinct event kinds present in the log, for the browser's filter
    /// menu — the catalog lists every kind the code can emit, but only the ones
    /// that actually happened are worth offering.
    pub(crate) async fn observed_kinds(&self) -> Vec<String> {
        let Some(pool) = self.pool.as_ref() else {
            return Vec::new();
        };
        match sqlx::query_as::<_, (String,)>("SELECT DISTINCT kind FROM events ORDER BY kind")
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows.into_iter().map(|(kind,)| kind).collect(),
            Err(err) => {
                error!(error = ?err, "failed to read the event kinds present in the log");
                Vec::new()
            }
        }
    }

    /// When each of `kinds` last happened to `instance`, keyed by kind.
    ///
    /// Returns empty when the log is disabled or the query fails: this backs a
    /// detail line in a tool result, so a database problem should cost that one
    /// line rather than fail the whole lookup. The failure is logged.
    pub(crate) async fn latest_per_kind(
        &self,
        instance: &str,
        kinds: &[EventKind],
    ) -> Vec<LatestEvent> {
        let Some(pool) = self.pool.as_ref() else {
            return Vec::new();
        };
        let wanted: Vec<&str> = kinds.iter().map(|kind| kind.as_str()).collect();
        match sqlx::query_as::<_, (String, String, String)>(LATEST_PER_KIND)
            .bind(instance)
            .bind(&wanted)
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows
                .into_iter()
                .map(|(kind, actor, created_at)| LatestEvent {
                    kind,
                    actor,
                    created_at,
                })
                .collect(),
            Err(err) => {
                error!(error = ?err, instance, "failed to read latest events for a server");
                Vec::new()
            }
        }
    }

    /// Append a lifecycle event, filling in the owning guild and game from the
    /// instance's labels when the caller doesn't already know them — warm
    /// actions (stop, restart, shutdown, destroy) are handed only a name.
    ///
    /// The label read is skipped entirely when the log is disabled, so a bot
    /// without a database makes no extra API calls on lifecycle paths.
    pub(crate) async fn record_lifecycle(
        &self,
        client: &Client,
        namespace: &str,
        mut event: ProductEvent,
    ) {
        if !self.enabled() {
            return;
        }
        if let Some(instance) = event.instance.clone()
            && (event.guild.is_none() || event.game.is_none())
        {
            match crate::agones::owner_in_cluster(client, namespace, instance.as_str()).await {
                Ok(Some(owner)) => {
                    event.guild = event.guild.or(owner.guild);
                    event.game = event.game.or(owner.game);
                }
                // A destroy has already removed the Service by the time it
                // reports, so an absent owner is expected, not a fault.
                Ok(None) => {}
                Err(err) => warn!(
                    error = ?err,
                    instance = instance.as_str(),
                    "could not resolve the owning guild and game for an event"
                ),
            }
        }
        self.record(event).await;
    }

    /// Append one sampler tick's occupancy readings. Like [`record`](Self::record),
    /// failure is logged and swallowed — a sample nobody is waiting on must never
    /// surface anywhere.
    pub(crate) async fn record_samples(&self, samples: &[OccupancySample]) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };
        if samples.is_empty() {
            return;
        }
        let instances: Vec<&str> = samples
            .iter()
            .map(|sample| sample.instance.as_str())
            .collect();
        let games: Vec<Option<&str>> = samples
            .iter()
            .map(|sample| sample.game.as_ref().map(GameId::as_str))
            .collect();
        let guilds: Vec<Option<&str>> = samples
            .iter()
            .map(|sample| sample.guild.as_ref().map(GuildId::as_str))
            .collect();
        let players: Vec<Option<i64>> = samples
            .iter()
            .map(|sample| sample.players.map(i64::from))
            .collect();
        let restarts: Vec<i64> = samples
            .iter()
            .map(|sample| i64::from(sample.restarts))
            .collect();

        let result = sqlx::query(INSERT_SAMPLES)
            .bind(&instances)
            .bind(&games)
            .bind(&guilds)
            .bind(&players)
            .bind(&restarts)
            .execute(pool)
            .await;
        if let Err(err) = result {
            error!(
                error = ?err,
                samples = samples.len(),
                "failed to record occupancy samples"
            );
        }
    }

    /// Append one sampler tick's resource readings. Same contract as
    /// [`record_samples`](Self::record_samples): failure is logged and swallowed.
    pub(crate) async fn record_resource_samples(&self, samples: &[ResourceSample]) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };
        if samples.is_empty() {
            return;
        }
        let instances: Vec<&str> = samples
            .iter()
            .map(|sample| sample.instance.as_str())
            .collect();
        let games: Vec<Option<&str>> = samples
            .iter()
            .map(|sample| sample.game.as_ref().map(GameId::as_str))
            .collect();
        let guilds: Vec<Option<&str>> = samples
            .iter()
            .map(|sample| sample.guild.as_ref().map(GuildId::as_str))
            .collect();
        let cpu: Vec<Option<i64>> = samples.iter().map(|sample| sample.cpu_millicores).collect();
        let mem: Vec<Option<i64>> = samples.iter().map(|sample| sample.mem_bytes).collect();
        let cpu_request: Vec<Option<i64>> = samples
            .iter()
            .map(|sample| sample.cpu_request_millicores)
            .collect();
        let mem_request: Vec<Option<i64>> = samples
            .iter()
            .map(|sample| sample.mem_request_bytes)
            .collect();
        let cpu_limit: Vec<Option<i64>> = samples
            .iter()
            .map(|sample| sample.cpu_limit_millicores)
            .collect();
        let mem_limit: Vec<Option<i64>> = samples
            .iter()
            .map(|sample| sample.mem_limit_bytes)
            .collect();
        let pvc: Vec<Option<i64>> = samples.iter().map(|sample| sample.pvc_bytes).collect();

        let result = sqlx::query(INSERT_RESOURCE_SAMPLES)
            .bind(&instances)
            .bind(&games)
            .bind(&guilds)
            .bind(&cpu)
            .bind(&mem)
            .bind(&cpu_request)
            .bind(&mem_request)
            .bind(&cpu_limit)
            .bind(&mem_limit)
            .bind(&pvc)
            .execute(pool)
            .await;
        if let Err(err) = result {
            error!(
                error = ?err,
                samples = samples.len(),
                "failed to record resource samples"
            );
        }
    }

    /// Reconcile the known-guilds set against the guilds this process is in,
    /// emitting nothing. Run once the gateway reports `Ready`, so the acquisition
    /// timeline starts honestly at deployment instead of backdating every guild
    /// the bot was already in as a join.
    pub(crate) async fn seed_known_guilds(&self, guilds: &[u64]) {
        let Some(pool) = self.pool.as_ref() else {
            return;
        };
        let ids: Vec<String> = guilds.iter().map(u64::to_string).collect();
        match sqlx::query(SEED_GUILDS).bind(&ids).execute(pool).await {
            Ok(_) => {
                self.seeded.store(true, Ordering::SeqCst);
                info!(guilds = ids.len(), "known guilds reconciled");
            }
            // Leaving `seeded` false is the safe failure: joins keep being claimed
            // silently rather than backdating a whole guild list as new.
            Err(err) => error!(error = ?err, "failed to reconcile known guilds"),
        }
    }

    /// Claim a guild as known, reporting whether this was its first-ever sighting.
    ///
    /// Always `false` before [`seed_known_guilds`](EventLog::seed_known_guilds)
    /// has run: the gateway replays `GuildCreate` for every guild on connect, and
    /// until the set is reconciled those replays are indistinguishable from
    /// joins. A genuine join in that sub-second window is recorded as known
    /// without an event — the honest trade against backdating every guild.
    pub(crate) async fn claim_guild(&self, guild: u64) -> bool {
        let Some(pool) = self.pool.as_ref() else {
            return false;
        };
        let claimed = sqlx::query(CLAIM_GUILD)
            .bind(guild.to_string())
            .fetch_optional(pool)
            .await;
        match claimed {
            Ok(row) => row.is_some() && self.seeded.load(Ordering::SeqCst),
            Err(err) => {
                error!(error = ?err, guild, "failed to claim guild as known");
                false
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
