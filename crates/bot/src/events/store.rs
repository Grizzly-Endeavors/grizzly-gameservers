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

const EVENT_LOG_MAX_CONNECTIONS: u32 = 2;

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

const INSERT_EVENT: &str = "INSERT INTO events \
    (kind, guild_id, game, instance, actor, surface, outcome, details) \
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8::jsonb)";

const RECENT_EVENTS: &str = "SELECT id, kind, guild_id, game, instance, actor, surface, outcome, \
    details::text, created_at::text FROM events \
    WHERE ($1::text IS NULL OR kind = $1) \
    AND ($2::text IS NULL OR guild_id = $2) \
    AND ($3::text IS NULL OR instance = $3) \
    AND ($4::text IS NULL OR surface = $4) \
    AND ($5::bigint IS NULL OR id < $5) \
    ORDER BY id DESC LIMIT $6";

#[derive(Clone, Debug, Default)]
pub(crate) struct EventFilter {
    pub(crate) kind: Option<String>,
    pub(crate) guild: Option<String>,
    pub(crate) instance: Option<String>,
    pub(crate) surface: Option<String>,
    pub(crate) before_id: Option<i64>,
}

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

const CLAIM_GUILD: &str =
    "INSERT INTO known_guilds (guild_id) VALUES ($1) ON CONFLICT DO NOTHING RETURNING guild_id";

const SEED_GUILDS: &str =
    "INSERT INTO known_guilds (guild_id) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING";

const INSERT_SAMPLES: &str = "INSERT INTO occupancy_samples \
    (instance, game, guild_id, players, restarts) \
    SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::bigint[], $5::bigint[])";

const INSERT_RESOURCE_SAMPLES: &str = "INSERT INTO resource_samples \
    (instance, game, guild_id, cpu_millicores, mem_bytes, cpu_request_millicores, \
    mem_request_bytes, cpu_limit_millicores, mem_limit_bytes, pvc_bytes) \
    SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::bigint[], $5::bigint[], \
    $6::bigint[], $7::bigint[], $8::bigint[], $9::bigint[], $10::bigint[])";

const LATEST_PER_KIND: &str = "SELECT DISTINCT ON (kind) kind, actor, created_at::text \
    FROM events WHERE instance = $1 AND kind = ANY($2) \
    ORDER BY kind, created_at DESC";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LatestEvent {
    pub(crate) kind: String,
    pub(crate) actor: String,
    pub(crate) created_at: String,
}

pub(crate) struct EventLog {
    pool: Option<PgPool>,
    seeded: AtomicBool,
}

impl EventLog {
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
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

    pub(crate) fn enabled(&self) -> bool {
        self.pool.is_some()
    }

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

    pub(crate) async fn recent(&self, filter: &EventFilter, limit: i64) -> Vec<EventRow> {
        let Some(pool) = self.pool.as_ref() else {
            return Vec::new();
        };
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
            Err(err) => error!(error = ?err, "failed to reconcile known guilds"),
        }
    }

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
