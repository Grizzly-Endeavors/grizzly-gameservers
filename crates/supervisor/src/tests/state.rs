use super::*;

const WINDOW: Duration = Duration::from_mins(5);
const THRESHOLD: u32 = 3;

#[test]
fn starts_running_and_coming_up() {
    let state = SupervisorState::new();
    assert_eq!(
        state.desired,
        DesiredState::Running,
        "starts desiring Running"
    );
    assert_eq!(state.phase, ProcessPhase::Starting, "starts in Starting");
    assert_eq!(state.pid, None, "no child yet");
}

#[test]
fn first_boot_goes_starting_then_running_on_ready() {
    let now = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1234, now);
    assert_eq!(state.phase, ProcessPhase::Starting, "not ready yet");
    assert_eq!(state.pid, Some(1234), "tracks the child pid");
    state.on_ready();
    assert_eq!(state.phase, ProcessPhase::Running, "ready flips to Running");
}

#[test]
fn warm_relaunch_is_starting_until_the_new_child_accepts() {
    let now = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1, now);
    state.on_ready();
    // Operator restarts: stop then start the child in place.
    state.on_restart_requested();
    state.on_started(2, now);
    assert_eq!(
        state.phase,
        ProcessPhase::Starting,
        "a warm relaunch is Starting until the new child is accepting again — Running must not report early"
    );
    assert!(
        state.is_ready(),
        "readiness stays sticky across the relaunch so Agones /ready is not re-signalled"
    );
    // This boot's readiness probe confirms the port is accepting again.
    state.on_ready();
    assert_eq!(
        state.phase,
        ProcessPhase::Running,
        "only once the new child accepts does the phase honestly report Running"
    );
}

#[test]
fn intentional_stop_settles_to_stopped() {
    let now = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(7, now);
    state.on_ready();
    state.on_stop_requested();
    assert_eq!(state.phase, ProcessPhase::Stopping, "stop is in flight");
    let disposition = state.on_child_exit(now, WINDOW, THRESHOLD);
    assert_eq!(
        disposition,
        ExitDisposition::Clean,
        "an exit while desired==Stopped is clean"
    );
    assert_eq!(state.phase, ProcessPhase::Stopped, "settles to Stopped");
    assert_eq!(state.pid, None, "no child while stopped");
}

#[test]
fn unexpected_exit_below_threshold_relaunches() {
    let now = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(7, now);
    state.on_ready();
    let disposition = state.on_child_exit(now, WINDOW, THRESHOLD);
    assert_eq!(
        disposition,
        ExitDisposition::Relaunch,
        "a single crash while desired==Running relaunches"
    );
    assert_eq!(state.phase, ProcessPhase::Starting, "coming back up");
    assert_eq!(state.status(now).restarts, 1, "counts the restart");
}

#[test]
fn repeated_crashes_in_window_escalate() {
    let base = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1, base);
    state.on_ready();

    let first = state.on_child_exit(base, WINDOW, THRESHOLD);
    let second = state.on_child_exit(base + Duration::from_secs(10), WINDOW, THRESHOLD);
    let third = state.on_child_exit(base + Duration::from_secs(20), WINDOW, THRESHOLD);

    assert_eq!(first, ExitDisposition::Relaunch, "crash 1 relaunches");
    assert_eq!(second, ExitDisposition::Relaunch, "crash 2 relaunches");
    assert_eq!(
        third,
        ExitDisposition::Escalate,
        "crash 3 within the window escalates"
    );
    assert_eq!(
        state.phase,
        ProcessPhase::Crashed,
        "phase is Crashed on escalate"
    );
}

#[test]
fn clearing_crash_history_restores_the_full_budget() {
    let base = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1, base);
    state.on_ready();

    state.on_child_exit(base, WINDOW, THRESHOLD);
    state.on_child_exit(base + Duration::from_secs(10), WINDOW, THRESHOLD);
    let escalated = state.on_child_exit(base + Duration::from_secs(20), WINDOW, THRESHOLD);
    assert_eq!(escalated, ExitDisposition::Escalate, "the budget is spent");

    // A repaired child booted and reached readiness, so the pod recovered.
    state.on_started(2, base + Duration::from_secs(30));
    state.on_ready();
    state.clear_crash_history();

    let after_recovery = state.on_child_exit(base + Duration::from_secs(40), WINDOW, THRESHOLD);
    assert_eq!(
        after_recovery,
        ExitDisposition::Relaunch,
        "a crash after recovery starts a fresh window rather than re-escalating at once"
    );
}

#[test]
fn crash_history_survives_readiness_that_is_not_a_recovery() {
    let base = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1, base);
    state.on_ready();

    // A game that dies partway into every boot still reaches readiness each
    // cycle; those readies must not refund the crash budget.
    state.on_child_exit(base, WINDOW, THRESHOLD);
    state.on_started(2, base + Duration::from_secs(5));
    state.on_ready();
    state.on_child_exit(base + Duration::from_secs(10), WINDOW, THRESHOLD);
    state.on_started(3, base + Duration::from_secs(15));
    state.on_ready();
    let third = state.on_child_exit(base + Duration::from_secs(20), WINDOW, THRESHOLD);

    assert_eq!(
        third,
        ExitDisposition::Escalate,
        "a slow crash loop still escalates"
    );
}

#[test]
fn crashes_outside_window_do_not_accumulate() {
    let base = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(1, base);
    state.on_ready();

    // Two crashes far enough apart that each is alone in the window.
    let first = state.on_child_exit(base, WINDOW, THRESHOLD);
    let later = base + WINDOW + Duration::from_mins(1);
    let second = state.on_child_exit(later, WINDOW, THRESHOLD);
    let third = state.on_child_exit(later + Duration::from_secs(1), WINDOW, THRESHOLD);

    assert_eq!(first, ExitDisposition::Relaunch, "first crash relaunches");
    assert_eq!(
        second,
        ExitDisposition::Relaunch,
        "the stale first crash was pruned, so this is alone in the window"
    );
    assert_eq!(third, ExitDisposition::Relaunch, "still under threshold");
}

#[test]
fn status_reports_uptime_from_start() {
    let base = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(42, base);
    state.on_ready();
    let status = state.status(base + Duration::from_secs(30));
    assert_eq!(status.uptime_seconds, 30, "uptime measured from launch");
    assert_eq!(status.pid, Some(42), "reports the live pid");
    assert!(status.ready, "ready flag set");
    assert_eq!(status.process, ProcessPhase::Running, "running phase");
}

#[test]
fn status_zeroes_uptime_while_stopped() {
    let now = Instant::now();
    let mut state = SupervisorState::new();
    state.on_started(42, now);
    state.on_ready();
    state.on_stop_requested();
    state.on_child_exit(now, WINDOW, THRESHOLD);
    let status = state.status(now + Duration::from_secs(5));
    assert_eq!(status.uptime_seconds, 0, "no uptime while stopped");
    assert_eq!(status.pid, None, "no pid while stopped");
    assert_eq!(status.process, ProcessPhase::Stopped, "stopped phase");
}

#[test]
fn a_refused_launch_reports_as_crashed_without_spending_the_crash_budget() {
    // `Crashed` because that is the word the whole recovery chain understands —
    // the bot's verify loop restores the file's snapshot on the strength of it.
    // The restart counter stays put because nothing restarted, and spending the
    // budget here would blind the escalation that exists for an unstable game.
    let mut state = SupervisorState::new();
    state.on_started(42, Instant::now());
    let before = state.status(Instant::now()).restarts;

    state.on_launch_failed();

    let status = state.status(Instant::now());
    assert_eq!(status.process, ProcessPhase::Crashed);
    assert_eq!(status.restarts, before);
    assert_eq!(status.pid, None);
    assert_eq!(status.uptime_seconds, 0);
}

#[test]
fn a_refused_launch_leaves_the_ready_flag_alone() {
    // The pod is still Ready to Agones and the control API is still up; only the
    // game process is missing. Clearing readiness here would make the bot's own
    // one-shot `/ready` fire again on the repair.
    let mut state = SupervisorState::new();
    state.on_ready();
    state.on_launch_failed();
    assert!(state.is_ready());
}

#[test]
fn a_repair_after_a_refused_launch_settles_back_to_running() {
    let mut state = SupervisorState::new();
    state.on_launch_failed();
    state.on_started(7, Instant::now());
    state.on_ready();
    assert_eq!(
        state.status(Instant::now()).process,
        ProcessPhase::Running,
        "a fixed mod list must leave no trace of the refusal"
    );
}
