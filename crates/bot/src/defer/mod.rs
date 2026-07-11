//! Gary's deferred-task queue: *wait until `{condition}` for `{server}`, then do
//! `{task}`*. The `run_when` tool enqueues a [`DeferredTask`] into Valkey and
//! ensures a background watcher is polling for that `(server, condition)`; when
//! the condition holds, the watcher drains every task queued for it and runs them
//! together as one manager-tier Gary turn, delivered back to the channel.
//!
//! Two design choices make this robust across the bot's frequent CI redeploys:
//! the queue is **durable** (in Valkey, not in memory), and watchers are **rebuilt
//! from Valkey on startup** ([`DeferRuntime::reconcile`]) — so a pending wait
//! survives a restart. Detection is **bot-side polling** of the supervisor's
//! existing `/status` and `/occupancy` endpoints (see [`watcher`]); the supervisor
//! is untouched.

mod condition;
mod task;
mod watcher;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

pub(crate) use condition::Condition;
pub(crate) use task::DeferredTask;

use condition::{KEY_SCAN_PATTERN, parse_wait_key, wait_key};

use crate::discord::Data;
use crate::kv::ValkeyClient;

/// TTL backstop on each queue key (24h), refreshed on every enqueue. Only reaps
/// keys a watcher somehow never drains; a normal play-session wait resolves long
/// before this, so it never truncates legitimate work.
const QUEUE_TTL_SECONDS: i64 = 24 * 60 * 60;

/// A watcher is identified by the `(server, condition)` it polls for.
type WatchKey = (String, Condition);

/// Owns the durable queue and the set of in-flight watchers. Held as an `Arc` on
/// [`Data`]; its methods take `&Data` + `&serenity::Context` at each spawn site so
/// it never stores `Data` (which would form a retain cycle through the `Arc`).
pub(crate) struct DeferRuntime {
    valkey: ValkeyClient,
    /// Which `(server, condition)` pairs currently have a live watcher, so a repeat
    /// enqueue doesn't spawn a second one. Best-effort dedup — correctness against
    /// a transient duplicate watcher rests on `LPOP`-atomic draining, not on this.
    watchers: Mutex<HashSet<WatchKey>>,
    /// Set once startup reconciliation has run, so the gateway's `Ready` event —
    /// which re-fires on every reconnect — rebuilds watchers only the first time.
    reconciled: AtomicBool,
}

impl DeferRuntime {
    pub(crate) fn new(valkey: ValkeyClient) -> Self {
        Self {
            valkey,
            watchers: Mutex::new(HashSet::new()),
            reconciled: AtomicBool::new(false),
        }
    }

    /// Whether the queue backend is usable. The `run_when` tool gates on this and
    /// reports it can't schedule when it's `false`.
    pub(crate) fn is_enabled(&self) -> bool {
        self.valkey.is_enabled()
    }

    /// Queue `task` to run for `server` once `condition` holds, and make sure a
    /// watcher is polling for it. Returns immediately — the wait happens in the
    /// background, so the calling Gary turn stays free.
    ///
    /// # Errors
    ///
    /// Returns an error if the task can't be serialized or the enqueue write fails.
    /// A failure to set the TTL backstop is logged, not propagated (the task is
    /// already durably queued).
    pub(crate) async fn enqueue_and_watch(
        &self,
        data: &Data,
        ctx: &serenity::Context,
        server: &str,
        condition: Condition,
        task: &DeferredTask,
    ) -> Result<()> {
        let key = wait_key(server, condition);
        let payload = serde_json::to_string(task).context("failed to serialize deferred task")?;
        self.valkey
            .rpush(&key, &payload)
            .await
            .context("failed to enqueue deferred task")?;
        if let Err(err) = self.valkey.expire(&key, QUEUE_TTL_SECONDS).await {
            warn!(error = ?err, key = %key, "failed to set deferred-queue ttl; task still queued");
        }
        self.ensure_watcher(data, ctx, server, condition);
        Ok(())
    }

    /// Rebuild watchers from the durable queue at startup, so a pending wait
    /// survives a bot redeploy. No-op when the backend is disabled.
    pub(crate) async fn reconcile(&self, data: &Data, ctx: &serenity::Context) {
        if !self.is_enabled() {
            return;
        }
        // `Ready` re-fires on every gateway reconnect; only the first rebuild is
        // meaningful (watchers persist across reconnects on the task tracker).
        if self.reconciled.swap(true, Ordering::SeqCst) {
            return;
        }
        let keys = match self.valkey.scan_keys(KEY_SCAN_PATTERN).await {
            Ok(keys) => keys,
            Err(err) => {
                error!(error = ?err, "failed to scan for pending deferred tasks; none re-armed");
                return;
            }
        };
        let mut rearmed = 0_usize;
        for key in &keys {
            if let Some((server, condition)) = parse_wait_key(key) {
                self.ensure_watcher(data, ctx, &server, condition);
                rearmed += 1;
            } else {
                warn!(key = %key, "skipping unrecognized deferred-task key during reconcile");
            }
        }
        if rearmed > 0 {
            info!(
                count = rearmed,
                "re-armed pending deferred-task watchers after startup"
            );
        }
    }

    /// Spawn a watcher for `(server, condition)` unless one is already registered.
    fn ensure_watcher(
        &self,
        data: &Data,
        ctx: &serenity::Context,
        server: &str,
        condition: Condition,
    ) {
        let newly_registered = self
            .watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((server.to_owned(), condition));
        if !newly_registered {
            return;
        }
        let tasks = data.tasks.clone();
        let (data, ctx, server) = (data.clone(), ctx.clone(), server.to_owned());
        tasks.spawn(async move {
            watcher::run_watcher(data, ctx, server, condition).await;
        });
    }

    /// Re-insert a watcher key without spawning — used by a running watcher that
    /// found a late arrival and is continuing its own loop rather than exiting.
    fn register_silent(&self, server: &str, condition: Condition) {
        self.watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((server.to_owned(), condition));
    }

    /// Drop a watcher key from the registry (the watcher is exiting or checking
    /// whether it may exit).
    fn deregister(&self, server: &str, condition: Condition) {
        self.watchers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&(server.to_owned(), condition));
    }
}
