//! Supervision for the scheduled background cycles.
//!
//! The stranded-instance reconciler, the occupancy sampler, the backup cycle and
//! the watchdog responder are plain spawned loops. Tokio isolates a panic to the
//! task that raised it, so the process survives — and the loop does not. Backups
//! or sampling would simply stop, with one panic line in the log and nothing
//! afterwards to say the subsystem was gone.
//!
//! Two things fix that: restarting a cycle whose task died, and letting a cycle
//! that *keeps* dying reach the liveness probe. A single panic is a bad tick, and
//! restarting the pod for one would be worse than the panic; a cycle that has
//! panicked repeatedly is not recovering on its own, which is exactly the case a
//! restart is for.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, warn};

/// How many times one cycle may panic before the process reports unhealthy.
/// Above one so a single bad tick doesn't cost a rollout, low enough that a cycle
/// stuck in a panic loop is caught within a few minutes.
const UNHEALTHY_PANIC_THRESHOLD: u32 = 3;

/// [`UNHEALTHY_PANIC_THRESHOLD`], for the health endpoint's tests — so they
/// assert against the real threshold rather than restating it.
#[cfg(test)]
pub(crate) const fn unhealthy_panic_threshold() -> u32 {
    UNHEALTHY_PANIC_THRESHOLD
}

/// How long to wait before restarting a cycle that panicked. A panic that repeats
/// immediately would otherwise spin the restart loop as fast as the runtime
/// allows, burying the log in its own retries.
const RESTART_BACKOFF: Duration = Duration::from_secs(5);

/// Panic counts per background cycle, shared with the health endpoint.
///
/// Counts are cumulative rather than consecutive: a cycle has no success signal
/// to reset against — its loop only returns when the process is shutting down —
/// so "panicked three times since boot" is the honest available measure.
#[derive(Default)]
pub(crate) struct CycleHealth {
    panics: Mutex<BTreeMap<&'static str, u32>>,
}

impl CycleHealth {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Count a panic without one having happened, so the health endpoint's
    /// behaviour can be tested without arranging a real panicking cycle.
    #[cfg(test)]
    pub(crate) fn record_panic_for_test(&self, cycle: &'static str) {
        self.record_panic(cycle);
    }

    fn record_panic(&self, cycle: &'static str) -> u32 {
        let Ok(mut panics) = self.panics.lock() else {
            // A poisoned map means a panic while holding it. The count is a
            // diagnostic, so losing it must not also lose the restart.
            warn!(cycle, "cycle health map is poisoned; panic not counted");
            return 0;
        };
        let count = panics.entry(cycle).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    /// The cycles that have panicked at least [`UNHEALTHY_PANIC_THRESHOLD`]
    /// times, newest count first — empty when the process is healthy.
    pub(crate) fn failing(&self) -> Vec<(&'static str, u32)> {
        let Ok(panics) = self.panics.lock() else {
            // Report healthy rather than restarting the pod on a poisoned lock:
            // the probe should fire on evidence, not on the absence of it.
            return Vec::new();
        };
        panics
            .iter()
            .filter(|(_, count)| **count >= UNHEALTHY_PANIC_THRESHOLD)
            .map(|(cycle, count)| (*cycle, *count))
            .collect()
    }
}

/// Run `body` as a supervised background cycle: if its task panics, log it, count
/// it, and start it again after a backoff.
///
/// `body` is a factory rather than a future because a restart needs a fresh one —
/// which is also why the handles a cycle closes over have to be cloneable.
pub(crate) fn spawn_supervised<F, Fut>(
    cycle: &'static str,
    health: &Arc<CycleHealth>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
    body: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let health = Arc::clone(health);
    tasks.spawn(async move {
        loop {
            // The inner spawn is what makes the panic observable: a panic inside
            // the tracked task itself would just end it, and JoinError is the only
            // thing that tells the difference between "finished" and "died".
            match tokio::spawn(body()).await {
                Err(err) if err.is_panic() => {
                    let count = health.record_panic(cycle);
                    error!(
                        cycle,
                        panics = count,
                        "background cycle panicked; restarting it"
                    );
                    if shutdown.is_cancelled() {
                        break;
                    }
                    tokio::select! {
                        () = sleep(RESTART_BACKOFF) => {}
                        () = shutdown.cancelled() => break,
                    }
                }
                // `Ok` is the cycle returning, which it only does at shutdown.
                // `Err` without a panic is cancellation — the runtime is going
                // away, so there is nothing left to restart into.
                Ok(()) | Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/cycles.rs"]
mod tests;
