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
