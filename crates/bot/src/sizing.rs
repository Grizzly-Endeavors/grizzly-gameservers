//! Per-instance CPU and memory overrides: what a server is allowed to use, as
//! distinct from what it is observed using (`events::resources`, the cost time
//! series). The model-facing tools call this a server's *resources*, which is
//! what a friend would say; internally it is sizing, so the two concerns don't
//! share a name.
//!
//! Overrides live in Postgres rather than on the Kubernetes object because a
//! cold start re-renders the `GameServer` from the catalog template
//! (`agones::provision::begin_start`), so anything not persisted outside the
//! cluster silently reverts the next time a server is stopped and started. The
//! façade mirrors [`crate::store::GuildConfig`]: loaded once at startup, cached
//! in memory, and **disabled** rather than failing when Postgres is unreachable.
//!
//! Unavailable is a refusal, not a fallback. A resize that can't be recorded
//! would apply to the running pod and then vanish on the next start, which is a
//! worse outcome than not resizing — so the tool refuses instead.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::config::DbConfig;
use crate::domain::InstanceName;
use crate::store::connect_pool;

/// Pool size for the sizing table. Written only when someone resizes a server
/// and read from the in-memory cache, so one spare connection is ample.
const SIZING_POOL_MAX_CONNECTIONS: u32 = 2;

/// Floor on a per-instance request/limit. Below this a game process either
/// won't start or thrashes, and "Gary shrank it until it broke" is a worse
/// failure than a refusal.
pub(crate) const MIN_CPU_MILLICORES: i64 = 250;
pub(crate) const MIN_MEM_BYTES: i64 = 512 * 1024 * 1024;

/// Ceiling on a per-instance request/limit. Sized to the largest node in the
/// cluster (32 cores / ~62Gi allocatable): anything above it cannot be
/// scheduled anywhere, so accepting it would only produce a pod that never
/// starts. This is the blast-radius bound on an agent driven by friends' free
/// text — the RBAC grant bounds the *shape* of what it can change, and this
/// bounds the values.
pub(crate) const MAX_CPU_MILLICORES: i64 = 16_000;
pub(crate) const MAX_MEM_BYTES: i64 = 32 * 1024 * 1024 * 1024;

const SIZING_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS instance_sizing (\
        instance TEXT PRIMARY KEY, \
        cpu_request_millicores BIGINT NOT NULL, \
        mem_request_bytes BIGINT NOT NULL, \
        cpu_limit_millicores BIGINT NOT NULL, \
        mem_limit_bytes BIGINT NOT NULL, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

/// One instance's declared CPU and memory, as requests and limits. Quantities in
/// the same units the resource sampler records (millicores and bytes), so the
/// declared and observed planes compare without conversion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Sizing {
    pub(crate) cpu_request_millicores: i64,
    pub(crate) mem_request_bytes: i64,
    pub(crate) cpu_limit_millicores: i64,
    pub(crate) mem_limit_bytes: i64,
}

/// Why a proposed sizing was refused. Each names the specific figure at fault so
/// the caller can say which one, rather than "invalid resources".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SizingRejection {
    CpuBelowFloor,
    MemoryBelowFloor,
    CpuAboveCeiling,
    MemoryAboveCeiling,
    /// A request above its own limit — Kubernetes rejects the pod outright, so
    /// catching it here turns an unschedulable server into a clear message.
    CpuRequestExceedsLimit,
    MemoryRequestExceedsLimit,
}

impl Sizing {
    /// Check a proposed sizing against the floors, ceilings, and the
    /// request-under-limit rule.
    ///
    /// Deliberately a refusal rather than a clamp: a friend told "done" who
    /// silently got something else is worse served than one told the number was
    /// out of range. The bounds are checked against the *limit* for the ceiling
    /// and the *request* for the floor, since those are the figures that decide
    /// schedulability and viability respectively.
    ///
    /// # Errors
    ///
    /// Returns the first violated rule, in the order declared above.
    pub(crate) fn validate(self) -> Result<(), SizingRejection> {
        if self.cpu_request_millicores < MIN_CPU_MILLICORES {
            return Err(SizingRejection::CpuBelowFloor);
        }
        if self.mem_request_bytes < MIN_MEM_BYTES {
            return Err(SizingRejection::MemoryBelowFloor);
        }
        if self.cpu_limit_millicores > MAX_CPU_MILLICORES {
            return Err(SizingRejection::CpuAboveCeiling);
        }
        if self.mem_limit_bytes > MAX_MEM_BYTES {
            return Err(SizingRejection::MemoryAboveCeiling);
        }
        if self.cpu_request_millicores > self.cpu_limit_millicores {
            return Err(SizingRejection::CpuRequestExceedsLimit);
        }
        if self.mem_request_bytes > self.mem_limit_bytes {
            return Err(SizingRejection::MemoryRequestExceedsLimit);
        }
        Ok(())
    }

    /// Render as Kubernetes quantity strings, for a pod spec or a resize patch.
    /// Millicores and plain bytes rather than `Gi`, because an exact integer
    /// never rounds and the API normalizes the display form anyway.
    pub(crate) fn as_quantities(self) -> SizingQuantities {
        SizingQuantities {
            cpu_request: format!("{}m", self.cpu_request_millicores),
            mem_request: self.mem_request_bytes.to_string(),
            cpu_limit: format!("{}m", self.cpu_limit_millicores),
            mem_limit: self.mem_limit_bytes.to_string(),
        }
    }
}

/// A [`Sizing`] in Kubernetes quantity form.
pub(crate) struct SizingQuantities {
    pub(crate) cpu_request: String,
    pub(crate) mem_request: String,
    pub(crate) cpu_limit: String,
    pub(crate) mem_limit: String,
}

/// A connection pool for the sizing table, schema applied.
struct SizingStore {
    pool: PgPool,
}

impl SizingStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, SIZING_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(SIZING_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply instance_sizing schema")?;
        Ok(Self { pool })
    }

    async fn load_all(&self) -> Result<HashMap<String, Sizing>> {
        let rows: Vec<(String, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT instance, cpu_request_millicores, mem_request_bytes, \
             cpu_limit_millicores, mem_limit_bytes FROM instance_sizing",
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to load instance sizing")?;
        Ok(rows
            .into_iter()
            .map(
                |(instance, cpu_request, mem_request, cpu_limit, mem_limit)| {
                    (
                        instance,
                        Sizing {
                            cpu_request_millicores: cpu_request,
                            mem_request_bytes: mem_request,
                            cpu_limit_millicores: cpu_limit,
                            mem_limit_bytes: mem_limit,
                        },
                    )
                },
            )
            .collect())
    }

    async fn upsert(&self, instance: &str, sizing: Sizing, actor: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO instance_sizing (instance, cpu_request_millicores, mem_request_bytes, \
             cpu_limit_millicores, mem_limit_bytes, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (instance) DO UPDATE SET \
             cpu_request_millicores = EXCLUDED.cpu_request_millicores, \
             mem_request_bytes = EXCLUDED.mem_request_bytes, \
             cpu_limit_millicores = EXCLUDED.cpu_limit_millicores, \
             mem_limit_bytes = EXCLUDED.mem_limit_bytes, \
             updated_at = now(), updated_by = EXCLUDED.updated_by",
        )
        .bind(instance)
        .bind(sizing.cpu_request_millicores)
        .bind(sizing.mem_request_bytes)
        .bind(sizing.cpu_limit_millicores)
        .bind(sizing.mem_limit_bytes)
        .bind(actor)
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to save sizing for {instance}"))?;
        Ok(())
    }

    async fn forget(&self, instance: &str) -> Result<()> {
        sqlx::query("DELETE FROM instance_sizing WHERE instance = $1")
            .bind(instance)
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to clear sizing for {instance}"))?;
        Ok(())
    }
}

/// The per-instance sizing overrides, cached in memory.
pub(crate) struct InstanceSizing {
    store: Option<SizingStore>,
    cache: RwLock<HashMap<String, Sizing>>,
}

impl InstanceSizing {
    /// Connect to Postgres (if configured), load every override, and return the
    /// façade. Never fails: any problem is logged and leaves the façade disabled,
    /// so servers keep running on their catalog sizing.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            return Self::disabled();
        };
        let store = match SizingStore::connect(config).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; per-instance resource overrides disabled");
                return Self::disabled();
            }
        };
        match store.load_all().await {
            Ok(cache) => {
                info!(
                    instances_sized = cache.len(),
                    "loaded per-instance resource overrides"
                );
                Self {
                    store: Some(store),
                    cache: RwLock::new(cache),
                }
            }
            Err(err) => {
                error!(error = ?err, "failed to load instance sizing; overrides disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            store: None,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Whether overrides can be recorded at all. `false` means a resize must be
    /// refused rather than applied — see the module docs.
    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    /// This instance's override, or `None` to use the catalog's sizing.
    pub(crate) async fn get(&self, instance: &str) -> Option<Sizing> {
        self.cache.read().await.get(instance).copied()
    }

    /// Record an override, replacing any earlier one.
    ///
    /// # Errors
    ///
    /// Returns an error if persistence is unavailable or the write fails. The
    /// cache is only updated after the write lands, so it never claims a sizing
    /// the database doesn't hold.
    pub(crate) async fn set(
        &self,
        instance: &InstanceName,
        sizing: Sizing,
        actor: &str,
    ) -> Result<()> {
        let store = self
            .store
            .as_ref()
            .context("resource overrides are unavailable (no database)")?;
        store.upsert(instance.as_str(), sizing, actor).await?;
        self.cache
            .write()
            .await
            .insert(instance.as_str().to_owned(), sizing);
        Ok(())
    }

    /// Drop an instance's override so it returns to the catalog sizing. Called
    /// when a server is destroyed, so a later server reusing the name doesn't
    /// silently inherit the dead one's resources.
    ///
    /// # Errors
    ///
    /// Returns an error if the delete fails. A no-op when persistence is
    /// unavailable: there is nothing cached to be wrong about.
    pub(crate) async fn forget(&self, instance: &str) -> Result<()> {
        let Some(store) = self.store.as_ref() else {
            return Ok(());
        };
        store.forget(instance).await?;
        self.cache.write().await.remove(instance);
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/sizing.rs"]
mod tests;
