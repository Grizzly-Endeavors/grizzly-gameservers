use anyhow::{Context, Result, anyhow};
use redis::aio::ConnectionManager;
use tracing::{error, info, warn};

use crate::config::ValkeyConfig;

#[derive(Clone)]
pub(crate) struct ValkeyClient {
    manager: Option<ConnectionManager>,
}

impl ValkeyClient {
    pub(crate) async fn connect(config: Option<&ValkeyConfig>) -> Self {
        let Some(config) = config else {
            warn!(
                "REDIS_PASSWORD not set; Gary's deferred-task queue (run_when) is disabled \
                 (everything else still works)"
            );
            return Self { manager: None };
        };
        match Self::try_connect(config).await {
            Ok(manager) => {
                info!(
                    host = %config.host,
                    port = config.port,
                    db = config.db,
                    "connected to foundation kv-cache (Valkey) for the deferred-task queue"
                );
                Self {
                    manager: Some(manager),
                }
            }
            Err(err) => {
                error!(
                    error = ?err,
                    host = %config.host,
                    port = config.port,
                    "valkey unavailable; deferred-task queue disabled (run_when will report it)"
                );
                Self { manager: None }
            }
        }
    }

    async fn try_connect(config: &ValkeyConfig) -> Result<ConnectionManager> {
        let client = redis::Client::open(config.url()).with_context(|| {
            format!(
                "failed to build redis client for {}:{}",
                config.host, config.port
            )
        })?;
        let mut manager = ConnectionManager::new(client).await.with_context(|| {
            format!(
                "failed to connect to valkey at {}:{}",
                config.host, config.port
            )
        })?;
        let _: () = redis::cmd("PING")
            .query_async(&mut manager)
            .await
            .context("valkey PING failed (bad password or unreachable)")?;
        Ok(manager)
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.manager.is_some()
    }

    fn conn(&self) -> Result<ConnectionManager> {
        self.manager
            .clone()
            .ok_or_else(|| anyhow!("valkey is not configured"))
    }

    pub(crate) async fn rpush(&self, key: &str, value: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let _: i64 = redis::cmd("RPUSH")
            .arg(key)
            .arg(value)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("failed to rpush to {key}"))?;
        Ok(())
    }

    pub(crate) async fn expire(&self, key: &str, seconds: i64) -> Result<()> {
        let mut conn = self.conn()?;
        let _: i64 = redis::cmd("EXPIRE")
            .arg(key)
            .arg(seconds)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("failed to set ttl on {key}"))?;
        Ok(())
    }

    pub(crate) async fn drain(&self, key: &str) -> Result<Vec<String>> {
        let mut conn = self.conn()?;
        let mut items = Vec::new();
        loop {
            let item: Option<String> = redis::cmd("LPOP")
                .arg(key)
                .query_async(&mut conn)
                .await
                .with_context(|| format!("failed to lpop from {key}"))?;
            match item {
                Some(value) => items.push(value),
                None => break,
            }
        }
        Ok(items)
    }

    pub(crate) async fn is_empty(&self, key: &str) -> Result<bool> {
        let mut conn = self.conn()?;
        let exists: bool = redis::cmd("EXISTS")
            .arg(key)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("failed to check existence of {key}"))?;
        Ok(!exists)
    }

    pub(crate) async fn scan_keys(&self, pattern: &str) -> Result<Vec<String>> {
        let mut conn = self.conn()?;
        let mut cursor: u64 = 0;
        let mut keys = Vec::new();
        loop {
            let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(200)
                .query_async(&mut conn)
                .await
                .with_context(|| format!("failed to scan for {pattern}"))?;
            keys.extend(batch);
            if next == 0 {
                break;
            }
            cursor = next;
        }
        Ok(keys)
    }
}

#[cfg(test)]
#[path = "tests/kv.rs"]
mod tests;
