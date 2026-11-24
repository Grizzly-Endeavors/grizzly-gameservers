use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, warn};

const UNHEALTHY_PANIC_THRESHOLD: u32 = 3;

#[cfg(test)]
pub(crate) const fn unhealthy_panic_threshold() -> u32 {
    UNHEALTHY_PANIC_THRESHOLD
}

const RESTART_BACKOFF: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(crate) struct CycleHealth {
    panics: Mutex<BTreeMap<&'static str, u32>>,
}

impl CycleHealth {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    #[cfg(test)]
    pub(crate) fn record_panic_for_test(&self, cycle: &'static str) {
        self.record_panic(cycle);
    }

    fn record_panic(&self, cycle: &'static str) -> u32 {
        let Ok(mut panics) = self.panics.lock() else {
            warn!(cycle, "cycle health map is poisoned; panic not counted");
            return 0;
        };
        let count = panics.entry(cycle).or_insert(0);
        *count = count.saturating_add(1);
        *count
    }

    pub(crate) fn failing(&self) -> Vec<(&'static str, u32)> {
        let Ok(panics) = self.panics.lock() else {
            return Vec::new();
        };
        panics
            .iter()
            .filter(|(_, count)| **count >= UNHEALTHY_PANIC_THRESHOLD)
            .map(|(cycle, count)| (*cycle, *count))
            .collect()
    }
}

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
                Ok(()) | Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
#[path = "tests/cycles.rs"]
mod tests;
