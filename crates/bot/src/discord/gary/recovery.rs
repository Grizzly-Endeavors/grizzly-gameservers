//! Deterministic crash recovery for a config change Gary applied to a live
//! server. The design's hard guardrail is "snapshot → apply → restart → verify →
//! auto-rollback, and an escalation exit" (`docs/design/00-overview.md`): a bad
//! value turns a restart into a crash, and recovery must not depend on the model
//! still having round budget or reasoning its way back to `restore_file`. So the
//! loop — not the model — watches the restart and reverts a crash.
//!
//! This file is the pure decision core: given how a (re)started server resolved,
//! it says what the loop should do next. The shell that actually polls readiness
//! and issues the restore/restart lives in `tools.rs`; keeping the decision here
//! makes the state machine unit-testable without a live cluster.

use crate::agones::ReadyWait;

/// A config edit Gary applied and then restarted into, still awaiting verification.
/// Set when `edit_file`/`write_file` saves a pre-edit snapshot (so `restore_file`
/// has something to fall back on); consumed by the restart that follows. Only the
/// last such edit is tracked — that mirrors `restore_file`, which itself only undoes
/// the most recent write to a given file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingChange {
    pub(super) server: String,
    pub(super) path: String,
}

/// The loop's next move after watching a restart that applied a config change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryStep {
    /// Came back up — the change is good; stop and report success.
    Healthy,
    /// Crashed with a snapshot still to fall back on — restore it and restart once.
    RollBack,
    /// Crashed after the one rollback was already spent — hand off to a human.
    Escalate,
    /// Neither up nor crashed (timed out, paused, or gone) — report as-is and
    /// leave the world untouched, since there's no crash to attribute to the change.
    Inconclusive,
}

/// Decide the next recovery move from how the readiness wait resolved.
///
/// `rollback_spent` bounds recovery to a single automatic restore: once one
/// rollback has been attempted, a still-crashing server escalates rather than
/// looping, because a crash the snapshot can't fix (or one the rollback itself
/// caused) is exactly the case a human needs to see.
pub(super) fn next_step(outcome: &ReadyWait, rollback_spent: bool) -> RecoveryStep {
    match outcome {
        ReadyWait::Ready => RecoveryStep::Healthy,
        ReadyWait::Crashed if rollback_spent => RecoveryStep::Escalate,
        ReadyWait::Crashed => RecoveryStep::RollBack,
        ReadyWait::Stopped | ReadyWait::TimedOut | ReadyWait::NotFound | ReadyWait::NotManaged => {
            RecoveryStep::Inconclusive
        }
    }
}

#[cfg(test)]
#[path = "tests/recovery.rs"]
mod tests;
