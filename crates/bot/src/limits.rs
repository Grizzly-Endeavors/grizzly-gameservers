//! Per-guild resource caps: how much of the cluster one Discord guild may hold.
//!
//! The per-instance clamp (`crate::sizing`) bounds a single runaway resize; this
//! module bounds the *aggregate* — how many servers a guild can have at once and
//! how much declared CPU/memory those servers may sum to. It exists for the
//! stranger beta: onboarding is gated, but no guild should be able to crowd the
//! others off the hardware, however it got in.
//!
//! Two halves, mirroring `crate::sizing`'s layout: pure cap math on top (what is
//! a guild's effective cap, does an action fit under it), and a Postgres façade
//! below holding per-guild overrides an operator can set. Usage is never read
//! from the database — callers measure it live off the cluster (see
//! `crate::agones::guild_usage`), so enforcement keeps working when Postgres is
//! down. Unavailable persistence therefore degrades to the *global defaults*
//! rather than to a refusal: unlike a sizing override, which records state that
//! would silently evaporate, a missing override row just means the guild is
//! capped at the generous baseline.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::config::{DbConfig, GuildLimitDefaults};
use crate::settings::RuntimeSettings;
use crate::sizing::Sizing;
use crate::store::connect_pool;

/// Pool size for the guild-limits table. Written only when the operator adjusts
/// a guild's caps and read from the in-memory cache, so one spare is ample.
const LIMITS_POOL_MAX_CONNECTIONS: u32 = 2;

const LIMITS_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS guild_limits (\
        guild_id TEXT PRIMARY KEY, \
        max_servers INT, \
        max_cpu_limit_millicores BIGINT, \
        max_mem_limit_bytes BIGINT, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

/// One guild's effective caps, after overrides are merged onto the global
/// defaults. CPU/memory caps bound the sum of declared *limits* across the
/// guild's live pods — limits are the blast radius a guild can inflict on the
/// hardware; requests are the scheduler's concern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildCaps {
    pub(crate) servers: u32,
    pub(crate) cpu_limit_millicores: i64,
    pub(crate) mem_limit_bytes: i64,
}

/// What a guild currently holds, measured live off the cluster. `server_count`
/// spans every existing instance — running, paused, or stopped, since each holds
/// a `NodePort` lease and a PVC — while the aggregate figures span only
/// instances with a live pod, because only those occupy CPU and memory.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuildUsage {
    pub(crate) server_count: usize,
    pub(crate) live_cpu_limit_millicores: i64,
    pub(crate) live_mem_limit_bytes: i64,
}

/// Why an action was refused, carrying the figures so every surface — embed,
/// tool result, event log — can name the numbers instead of saying "limit".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LimitRefusal {
    ServerCount {
        current: usize,
        cap: u32,
    },
    AggregateCpu {
        current_millicores: i64,
        adding_millicores: i64,
        cap_millicores: i64,
    },
    AggregateMemory {
        current_bytes: i64,
        adding_bytes: i64,
        cap_bytes: i64,
    },
}

/// One guild's override row. A `None` column means "use the global default" —
/// per column, so the operator can raise just the server count for a guild
/// without freezing its CPU/memory caps at today's defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuildLimitOverrides {
    pub(crate) servers: Option<u32>,
    pub(crate) cpu_limit_millicores: Option<i64>,
    pub(crate) mem_limit_bytes: Option<i64>,
}

/// A single-column adjustment to a guild's overrides. `None` clears the column
/// back to the global default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LimitChange {
    Servers(Option<u32>),
    CpuLimitMillicores(Option<i64>),
    MemLimitBytes(Option<i64>),
}

/// Merge a guild's overrides (if any) onto the global defaults.
pub(crate) fn effective_caps(
    overrides: Option<GuildLimitOverrides>,
    defaults: GuildLimitDefaults,
) -> GuildCaps {
    let overrides = overrides.unwrap_or_default();
    GuildCaps {
        servers: overrides.servers.unwrap_or(defaults.servers),
        cpu_limit_millicores: overrides
            .cpu_limit_millicores
            .unwrap_or(defaults.cpu_limit_millicores),
        mem_limit_bytes: overrides
            .mem_limit_bytes
            .unwrap_or(defaults.mem_limit_bytes),
    }
}

/// Check a new instance against the guild's caps: the server count first, then
/// the aggregate with the newcomer's declared limits added.
///
/// `adding` is the sizing the instance will render with (its override or the
/// catalog's). `None` — a catalog template whose figures couldn't be read —
/// contributes zero: deliberately permissive, because refusing every create over
/// an unreadable template would turn a catalog bug into a guild-wide outage.
///
/// # Errors
///
/// Returns the first violated cap with its figures.
pub(crate) fn check_create(
    usage: &GuildUsage,
    adding: Option<Sizing>,
    caps: &GuildCaps,
) -> Result<(), LimitRefusal> {
    let count = u64::try_from(usage.server_count).unwrap_or(u64::MAX);
    if count >= u64::from(caps.servers) {
        return Err(LimitRefusal::ServerCount {
            current: usage.server_count,
            cap: caps.servers,
        });
    }
    let (cpu, mem) = adding.map_or((0, 0), |sizing| {
        (sizing.cpu_limit_millicores, sizing.mem_limit_bytes)
    });
    check_aggregate(usage, cpu, mem, caps)
}

/// Check load being added to a guild's live aggregate — a cold start bringing a
/// stopped server's pod back, or a resize raising a live one. For a resize, pass
/// usage measured with the target instance excluded, so the proposal replaces
/// (rather than stacks onto) the target's current contribution.
///
/// # Errors
///
/// Returns the first violated cap with its figures.
pub(crate) fn check_added_load(
    usage: &GuildUsage,
    adding: Sizing,
    caps: &GuildCaps,
) -> Result<(), LimitRefusal> {
    check_aggregate(
        usage,
        adding.cpu_limit_millicores,
        adding.mem_limit_bytes,
        caps,
    )
}

fn check_aggregate(
    usage: &GuildUsage,
    adding_millicores: i64,
    adding_bytes: i64,
    caps: &GuildCaps,
) -> Result<(), LimitRefusal> {
    if usage
        .live_cpu_limit_millicores
        .saturating_add(adding_millicores)
        > caps.cpu_limit_millicores
    {
        return Err(LimitRefusal::AggregateCpu {
            current_millicores: usage.live_cpu_limit_millicores,
            adding_millicores,
            cap_millicores: caps.cpu_limit_millicores,
        });
    }
    if usage.live_mem_limit_bytes.saturating_add(adding_bytes) > caps.mem_limit_bytes {
        return Err(LimitRefusal::AggregateMemory {
            current_bytes: usage.live_mem_limit_bytes,
            adding_bytes,
            cap_bytes: caps.mem_limit_bytes,
        });
    }
    Ok(())
}

/// Millicores as a friendly core count: whole cores as `6 cores`, fractional as
/// `1.5 cores`, sub-core as `500m`. One home so embeds and tool results speak
/// the same figures.
pub(crate) fn format_cores(millicores: i64) -> String {
    if millicores >= 1000 {
        let whole = millicores / 1000;
        let tenths = (millicores % 1000) / 100;
        if millicores % 1000 == 0 {
            if whole == 1 {
                "1 core".to_owned()
            } else {
                format!("{whole} cores")
            }
        } else {
            format!("{whole}.{tenths} cores")
        }
    } else {
        format!("{millicores}m")
    }
}

/// One `guild_limits` row as it comes off the wire: guild id, then the three
/// nullable cap columns in schema order.
type OverrideRow = (String, Option<i32>, Option<i64>, Option<i64>);

/// A connection pool for the guild-limits table, schema applied.
struct LimitsStore {
    pool: PgPool,
}

impl LimitsStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, LIMITS_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(LIMITS_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply guild_limits schema")?;
        Ok(Self { pool })
    }

    async fn load_all(&self) -> Result<HashMap<String, GuildLimitOverrides>> {
        let rows: Vec<OverrideRow> = sqlx::query_as(
            "SELECT guild_id, max_servers, max_cpu_limit_millicores, max_mem_limit_bytes \
             FROM guild_limits",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to load guild limits")?;
        Ok(rows
            .into_iter()
            .map(|(guild, servers, cpu, mem)| {
                (
                    guild,
                    GuildLimitOverrides {
                        servers: servers.and_then(|value| u32::try_from(value).ok()),
                        cpu_limit_millicores: cpu,
                        mem_limit_bytes: mem,
                    },
                )
            })
            .collect())
    }

    async fn apply(&self, guild: &str, change: LimitChange, actor: &str) -> Result<()> {
        let query = match change {
            LimitChange::Servers(value) => sqlx::query(
                "INSERT INTO guild_limits (guild_id, max_servers, updated_by) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (guild_id) DO UPDATE SET \
                 max_servers = EXCLUDED.max_servers, \
                 updated_at = now(), updated_by = EXCLUDED.updated_by",
            )
            .bind(guild)
            .bind(value.and_then(|servers| i32::try_from(servers).ok()))
            .bind(actor),
            LimitChange::CpuLimitMillicores(value) => sqlx::query(
                "INSERT INTO guild_limits (guild_id, max_cpu_limit_millicores, updated_by) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (guild_id) DO UPDATE SET \
                 max_cpu_limit_millicores = EXCLUDED.max_cpu_limit_millicores, \
                 updated_at = now(), updated_by = EXCLUDED.updated_by",
            )
            .bind(guild)
            .bind(value)
            .bind(actor),
            LimitChange::MemLimitBytes(value) => sqlx::query(
                "INSERT INTO guild_limits (guild_id, max_mem_limit_bytes, updated_by) \
                 VALUES ($1, $2, $3) \
                 ON CONFLICT (guild_id) DO UPDATE SET \
                 max_mem_limit_bytes = EXCLUDED.max_mem_limit_bytes, \
                 updated_at = now(), updated_by = EXCLUDED.updated_by",
            )
            .bind(guild)
            .bind(value)
            .bind(actor),
        };
        query
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to save guild limits for {guild}"))?;
        Ok(())
    }
}

/// The per-guild cap overrides plus the global defaults, cached in memory.
pub(crate) struct GuildLimits {
    store: Option<LimitsStore>,
    cache: RwLock<HashMap<String, GuildLimitOverrides>>,
    /// The global defaults, read live rather than copied: they are an operator
    /// setting, and a cached copy is how "raise everyone's cap" would silently
    /// need a restart.
    settings: Arc<RuntimeSettings>,
}

impl GuildLimits {
    /// Connect to Postgres (if configured), load every override, and return the
    /// façade. Never fails: any problem is logged and leaves the façade
    /// disabled, in which case every guild is capped at the global defaults.
    pub(crate) async fn connect(config: Option<&DbConfig>, settings: Arc<RuntimeSettings>) -> Self {
        let Some(config) = config else {
            return Self::disabled(settings);
        };
        let store = match LimitsStore::connect(config).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; per-guild cap overrides disabled");
                return Self::disabled(settings);
            }
        };
        match store.load_all().await {
            Ok(cache) => {
                info!(
                    guilds_overridden = cache.len(),
                    "loaded per-guild cap overrides"
                );
                Self {
                    store: Some(store),
                    cache: RwLock::new(cache),
                    settings,
                }
            }
            Err(err) => {
                error!(error = ?err, "failed to load guild limits; overrides disabled");
                Self::disabled(settings)
            }
        }
    }

    fn disabled(settings: Arc<RuntimeSettings>) -> Self {
        Self {
            store: None,
            cache: RwLock::new(HashMap::new()),
            settings,
        }
    }

    /// The global defaults every guild starts from, before its own overrides.
    pub(crate) fn defaults(&self) -> GuildLimitDefaults {
        self.settings.snapshot().guild_limits
    }

    /// Whether overrides can be recorded. `false` means an adjustment must be
    /// refused; enforcement itself keeps working on the global defaults.
    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    /// This guild's override row, or `None` when it runs on pure defaults.
    pub(crate) async fn overrides_for(&self, guild: &str) -> Option<GuildLimitOverrides> {
        self.cache.read().await.get(guild).copied()
    }

    /// This guild's effective caps: overrides merged onto the global defaults.
    pub(crate) async fn caps_for(&self, guild: &str) -> GuildCaps {
        effective_caps(self.overrides_for(guild).await, self.defaults())
    }

    /// Record a single-column adjustment, replacing any earlier value.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence is unavailable or the write fails. The
    /// cache is only updated after the write lands, so it never claims an
    /// override the database doesn't hold.
    pub(crate) async fn apply(&self, guild: &str, change: LimitChange, actor: &str) -> Result<()> {
        let store = self
            .store
            .as_ref()
            .context("guild limit overrides are unavailable (no database)")?;
        store.apply(guild, change, actor).await?;
        let mut cache = self.cache.write().await;
        let entry = cache.entry(guild.to_owned()).or_default();
        match change {
            LimitChange::Servers(value) => entry.servers = value,
            LimitChange::CpuLimitMillicores(value) => entry.cpu_limit_millicores = value,
            LimitChange::MemLimitBytes(value) => entry.mem_limit_bytes = value,
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/limits.rs"]
mod tests;
