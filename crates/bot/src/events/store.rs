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

use super::catalog::ProductEvent;
use crate::config::DbConfig;
use crate::domain::InstanceName;

/// Postgres pool size for the event log. Two is ample: writes are one small
/// insert per product action at friends scale, never a hot path.
const EVENT_LOG_MAX_CONNECTIONS: u32 = 2;

/// Idempotent schema for both tables of the product plane.
///
/// One wide `events` table rather than a table per action: the catalog is closed
/// and every row answers the same question ("who did what, where, and how did it
/// end"), so the derivation layer's queries are `WHERE kind = ...` rather than a
/// union across a dozen shapes. `guild_id`, `game`, and `instance` are nullable
/// because not every kind has all three.
///
/// `known_guilds` exists so `guild_joined` survives a gateway reconnect, which
/// replays `GuildCreate` for every guild the bot is already in.
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
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now())";

/// `details` is bound as text and cast, because sqlx is pinned without its `json`
/// feature — the cast costs nothing and keeps the dependency surface as it is.
const INSERT_EVENT: &str = "INSERT INTO events \
    (kind, guild_id, game, instance, actor, surface, outcome, details) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)";

/// Claim a guild as known. `RETURNING` fires only on a real insert, so the row
/// count *is* the answer to "is this a genuine join?" — no read-then-write race.
const CLAIM_GUILD: &str =
    "INSERT INTO known_guilds (guild_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING guild_id";

const SEED_GUILDS: &str =
    "INSERT INTO known_guilds (guild_id) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING";

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
            .bind(event.guild.as_ref().map(crate::domain::GuildId::as_str))
            .bind(event.game.as_ref().map(crate::domain::GameId::as_str))
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
            match crate::agones::owner_of(client, namespace, instance.as_str()).await {
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
