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

const LIMITS_POOL_MAX_CONNECTIONS: u32 = 2;

const LIMITS_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS guild_limits (\
        guild_id TEXT PRIMARY KEY, \
        max_servers INT, \
        max_cpu_limit_millicores BIGINT, \
        max_mem_limit_bytes BIGINT, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildCaps {
    pub(crate) servers: u32,
    pub(crate) cpu_limit_millicores: i64,
    pub(crate) mem_limit_bytes: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuildUsage {
    pub(crate) server_count: usize,
    pub(crate) live_cpu_limit_millicores: i64,
    pub(crate) live_mem_limit_bytes: i64,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GuildLimitOverrides {
    pub(crate) servers: Option<u32>,
    pub(crate) cpu_limit_millicores: Option<i64>,
    pub(crate) mem_limit_bytes: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LimitChange {
    Servers(Option<u32>),
    CpuLimitMillicores(Option<i64>),
    MemLimitBytes(Option<i64>),
}

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

pub(crate) fn human_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        scaled_size(bytes, GIB, "GiB")
    } else if bytes >= MIB {
        scaled_size(bytes, MIB, "MiB")
    } else if bytes >= KIB {
        scaled_size(bytes, KIB, "KiB")
    } else {
        format!("{bytes} B")
    }
}

fn scaled_size(bytes: u64, divisor: u64, unit: &str) -> String {
    let whole = bytes / divisor;
    let tenths = (bytes % divisor) * 10 / divisor;
    format!("{whole}.{tenths} {unit}")
}

type OverrideRow = (String, Option<i32>, Option<i64>, Option<i64>);

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

pub(crate) struct GuildLimits {
    store: Option<LimitsStore>,
    cache: RwLock<HashMap<String, GuildLimitOverrides>>,
    settings: Arc<RuntimeSettings>,
}

impl GuildLimits {
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

    pub(crate) fn defaults(&self) -> GuildLimitDefaults {
        self.settings.snapshot().guild_limits
    }

    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) async fn overrides_for(&self, guild: &str) -> Option<GuildLimitOverrides> {
        self.cache.read().await.get(guild).copied()
    }

    pub(crate) async fn caps_for(&self, guild: &str) -> GuildCaps {
        effective_caps(self.overrides_for(guild).await, self.defaults())
    }

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
