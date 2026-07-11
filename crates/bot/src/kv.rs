//! The bot's client for the shared foundation kv-cache (Valkey) — the durable
//! store behind Gary's deferred-task queue (`run_when`). Like the Postgres façades
//! in [`crate::store`], it **degrades gracefully**: if `REDIS_PASSWORD` is unset or
//! Valkey is unreachable at startup, the client is *disabled* and every operation
//! reports it, so the bot still runs and only the deferred-task feature goes dark
//! (Gary tells the user he can't schedule) rather than crash-looping.
//!
//! The queue is a Redis list per `(server, condition)`; entries are JSON
//! [`crate::defer::DeferredTask`] payloads. All commands go through `redis::cmd`
//! (not the typed `AsyncCommands` trait) to stay stable across client versions.
//! `ConnectionManager` is an auto-reconnecting, cloneable, multiplexed connection,
//! so each call clones it for an owned `&mut` without a pool.

use anyhow::{Context, Result, anyhow};
use redis::aio::ConnectionManager;
use tracing::{error, info, warn};

use crate::config::ValkeyConfig;

/// A connection to the foundation Valkey, or a disabled stub when the store isn't
/// configured/reachable. Cheap to clone (the manager is an `Arc` internally).
#[derive(Clone)]
pub(crate) struct ValkeyClient {
    manager: Option<ConnectionManager>,
}

impl ValkeyClient {
    /// Connect to Valkey (if configured) and confirm it answers, or return a
    /// disabled client. Never fails: any problem is logged and leaves the queue
    /// backend off so the rest of the bot keeps working.
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
        // Establishes AUTH/reachability up front rather than discovering it on the
        // first real command mid-session.
        let _: () = redis::cmd("PING")
            .query_async(&mut manager)
            .await
            .context("valkey PING failed (bad password or unreachable)")?;
        Ok(manager)
    }

    /// Whether the queue backend is usable. Callers gate on this and report a
    /// friendly "can't schedule right now" when it's `false`.
    pub(crate) fn is_enabled(&self) -> bool {
        self.manager.is_some()
    }

    fn conn(&self) -> Result<ConnectionManager> {
        self.manager
            .clone()
            .ok_or_else(|| anyhow!("valkey is not configured"))
    }

    /// Append a task payload to `key`'s list (creating it if absent).
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the command fails.
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

    /// Set a TTL backstop (in seconds) on `key`, refreshing it. Keeps an orphaned
    /// queue from lingering forever if a watcher somehow never drains it.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the command fails.
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

    /// Pop every element of `key`'s list, in order, leaving it empty. Uses a
    /// per-element `LPOP` loop: each pop is atomic, so a task pushed by a
    /// concurrent enqueue is never lost or double-taken even if two drainers race.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or a pop fails.
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

    /// Whether `key`'s list is empty (an empty Redis list has no key).
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the command fails.
    pub(crate) async fn is_empty(&self, key: &str) -> Result<bool> {
        let mut conn = self.conn()?;
        let exists: bool = redis::cmd("EXISTS")
            .arg(key)
            .query_async(&mut conn)
            .await
            .with_context(|| format!("failed to check existence of {key}"))?;
        Ok(!exists)
    }

    /// Every key matching `pattern`, via a cursor `SCAN` (never `KEYS`, which
    /// blocks the shared instance). Used once at startup to rebuild watchers.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or a scan step fails.
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
