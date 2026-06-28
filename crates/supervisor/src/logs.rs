//! A bounded ring buffer of the supervised child's most recent output lines.
//!
//! The runner pipes the game process's stdout/stderr through here so the control
//! API can serve a tail of recent output to the ops agent — the one log source
//! that exists for *every* game, regardless of how it stores its own log files.
//! Lines are still re-emitted to the supervisor's own stdout, so `kubectl logs`
//! on the pod is unaffected.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

/// Lines returned by `GET /logs` when the caller doesn't specify a count, and
/// the ceiling the buffer retains. A few hundred lines is enough for the agent
/// to spot a crash or a config-rejection without flooding its context.
pub const DEFAULT_TAIL_LINES: usize = 200;

/// How many lines the buffer holds before dropping the oldest. Larger than the
/// default tail so a caller can ask for more history than one screenful.
const CAPACITY: usize = 1000;

/// A thread-safe, fixed-capacity buffer of recent output lines. Cheap to clone
/// behind an `Arc`; the reader task pushes and the control API tails, both under
/// a short-lived lock never held across an await.
#[derive(Debug)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<String>>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl LogBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(CAPACITY)),
        }
    }

    /// Append one line, dropping the oldest if the buffer is at capacity. A
    /// poisoned lock is recovered rather than propagated — a panic in another
    /// holder must not silence logging.
    pub fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The most recent `count` lines, oldest first. `count` is clamped to what
    /// the buffer holds.
    #[must_use]
    pub fn tail(&self, count: usize) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        let start = lines.len().saturating_sub(count);
        lines.iter().skip(start).cloned().collect()
    }
}

#[cfg(test)]
#[path = "tests/logs.rs"]
mod tests;
