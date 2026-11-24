use crate::agones::ReadyWait;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingChange {
    pub(super) server: String,
    pub(super) path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RecoveryStep {
    Healthy,
    RollBack,
    Escalate,
    Inconclusive,
}

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
