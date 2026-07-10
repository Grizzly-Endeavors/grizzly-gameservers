use super::*;

#[test]
fn healthy_server_is_verified_regardless_of_rollback_state() {
    assert_eq!(next_step(&ReadyWait::Ready, false), RecoveryStep::Healthy);
    // After a rollback, a healthy server means the rollback recovered it — still
    // "healthy" as far as the state machine is concerned; the shell picks the copy.
    assert_eq!(next_step(&ReadyWait::Ready, true), RecoveryStep::Healthy);
}

#[test]
fn first_crash_rolls_back() {
    assert_eq!(
        next_step(&ReadyWait::Crashed, false),
        RecoveryStep::RollBack
    );
}

#[test]
fn crash_after_rollback_escalates_instead_of_looping() {
    assert_eq!(next_step(&ReadyWait::Crashed, true), RecoveryStep::Escalate);
}

#[test]
fn non_crash_non_ready_outcomes_are_inconclusive() {
    for outcome in [
        ReadyWait::Stopped,
        ReadyWait::TimedOut,
        ReadyWait::NotFound,
        ReadyWait::NotManaged,
    ] {
        assert_eq!(next_step(&outcome, false), RecoveryStep::Inconclusive);
        assert_eq!(next_step(&outcome, true), RecoveryStep::Inconclusive);
    }
}

#[test]
fn recovery_is_bounded_to_a_single_rollback() {
    // The only step that loops back for another poll is RollBack, and it is only
    // reachable while rollback is unspent. Once spent, no input returns RollBack —
    // so the shell's verify loop can run at most twice.
    for outcome in [
        ReadyWait::Ready,
        ReadyWait::Crashed,
        ReadyWait::Stopped,
        ReadyWait::TimedOut,
        ReadyWait::NotFound,
        ReadyWait::NotManaged,
    ] {
        assert_ne!(next_step(&outcome, true), RecoveryStep::RollBack);
    }
}
