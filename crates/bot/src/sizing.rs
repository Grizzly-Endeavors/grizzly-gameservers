use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::config::DbConfig;
use crate::domain::InstanceName;
use crate::store::connect_pool;

const SIZING_POOL_MAX_CONNECTIONS: u32 = 2;

pub(crate) const MIN_CPU_MILLICORES: i64 = 250;
pub(crate) const MIN_MEM_BYTES: i64 = 512 * 1024 * 1024;

pub(crate) const MAX_CPU_MILLICORES: i64 = 16_000;
pub(crate) const MAX_MEM_BYTES: i64 = 32 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SizingBounds {
    pub(crate) min_cpu_millicores: i64,
    pub(crate) min_mem_bytes: i64,
    pub(crate) max_cpu_millicores: i64,
    pub(crate) max_mem_bytes: i64,
}

impl Default for SizingBounds {
    fn default() -> Self {
        Self {
            min_cpu_millicores: MIN_CPU_MILLICORES,
            min_mem_bytes: MIN_MEM_BYTES,
            max_cpu_millicores: MAX_CPU_MILLICORES,
            max_mem_bytes: MAX_MEM_BYTES,
        }
    }
}

impl SizingBounds {
    pub(crate) fn is_coherent(self) -> bool {
        self.min_cpu_millicores <= self.max_cpu_millicores
            && self.min_mem_bytes <= self.max_mem_bytes
    }
}

const SIZING_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS instance_sizing (\
        instance TEXT PRIMARY KEY, \
        cpu_request_millicores BIGINT NOT NULL, \
        mem_request_bytes BIGINT NOT NULL, \
        cpu_limit_millicores BIGINT NOT NULL, \
        mem_limit_bytes BIGINT NOT NULL, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

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
    pub(crate) fn validate(self, bounds: SizingBounds) -> Result<(), SizingRejection> {
        if self.cpu_request_millicores < bounds.min_cpu_millicores {
            return Err(SizingRejection::CpuBelowFloor);
        }
        if self.mem_request_bytes < bounds.min_mem_bytes {
            return Err(SizingRejection::MemoryBelowFloor);
        }
        if self.cpu_limit_millicores > bounds.max_cpu_millicores {
            return Err(SizingRejection::CpuAboveCeiling);
        }
        if self.mem_limit_bytes > bounds.max_mem_bytes {
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

pub(crate) struct SizingQuantities {
    pub(crate) cpu_request: String,
    pub(crate) mem_request: String,
    pub(crate) cpu_limit: String,
    pub(crate) mem_limit: String,
}

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

pub(crate) struct InstanceSizing {
    store: Option<SizingStore>,
    cache: RwLock<HashMap<String, Sizing>>,
}

impl InstanceSizing {
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

    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) async fn get(&self, instance: &str) -> Option<Sizing> {
        self.cache.read().await.get(instance).copied()
    }

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
