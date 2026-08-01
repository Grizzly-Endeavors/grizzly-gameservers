//! The Agones health heartbeat's on/off gate.
//!
//! The supervisor pings Agones `/health` on a cadence even while the game is
//! intentionally paused, so a pod is only torn down when the supervisor *decides*
//! it should be. That decision is crash escalation: enough crashes inside the
//! crash window and the heartbeat goes quiet, letting Agones recreate the pod.
//!
//! The gate is a switch rather than a one-way stop because escalation has to be
//! survivable. The bot's guardrail loop answers a failed config change by
//! restoring the file's snapshot and restarting; that repair is worthless if the
//! pod is already condemned. A relaunched child that reaches readiness is a
//! genuinely good boot, so the heartbeat resumes and the pod lives. A child that
//! keeps crashing never reaches readiness, so it is still condemned exactly as
//! before.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared switch for the Agones health heartbeat. Cloning shares the switch, so
/// the supervision loop and the spawned heartbeat task see each other's changes.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    beating: Arc<AtomicBool>,
}

impl Heartbeat {
    /// A heartbeat that starts out beating.
    #[must_use]
    pub fn new() -> Self {
        Self {
            beating: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Whether the heartbeat task should ping on its next tick.
    #[must_use]
    pub fn is_beating(&self) -> bool {
        self.beating.load(Ordering::SeqCst)
    }

    /// Go quiet, so Agones stops seeing health pings and recreates the pod unless
    /// the game recovers first.
    pub fn suspend(&self) {
        self.beating.store(false, Ordering::SeqCst);
    }

    /// Start pinging again after a recovery.
    pub fn resume(&self) {
        self.beating.store(true, Ordering::SeqCst);
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/heartbeat.rs"]
mod tests;
