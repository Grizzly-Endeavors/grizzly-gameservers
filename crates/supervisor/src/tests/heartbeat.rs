use super::*;

#[test]
fn starts_beating() {
    assert!(Heartbeat::new().is_beating());
}

#[test]
fn suspend_stops_it_and_resume_starts_it_again() {
    let heartbeat = Heartbeat::new();
    heartbeat.suspend();
    assert!(!heartbeat.is_beating());
    heartbeat.resume();
    assert!(heartbeat.is_beating());
}

#[test]
fn repeated_calls_are_idempotent() {
    let heartbeat = Heartbeat::new();
    heartbeat.suspend();
    heartbeat.suspend();
    assert!(!heartbeat.is_beating());
    heartbeat.resume();
    heartbeat.resume();
    assert!(heartbeat.is_beating());
}

#[test]
fn clones_share_the_switch() {
    let heartbeat = Heartbeat::new();
    let task_handle = heartbeat.clone();
    heartbeat.suspend();
    assert!(!task_handle.is_beating());
    task_handle.resume();
    assert!(heartbeat.is_beating());
}
