use super::*;

fn policy() -> AutoUpdatePolicy {
    AutoUpdatePolicy {
        empty_grace: Duration::from_mins(5),
        update_interval: Duration::from_hours(24),
        max_uptime: Duration::from_hours(72),
    }
}

const DAY: Duration = Duration::from_hours(24);

#[test]
fn empty_and_old_enough_after_grace_triggers_idle() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    assert_eq!(updater.observe(Some(0), 2 * DAY, t0), None);
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(299)),
        None
    );
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(301)),
        Some(RelaunchReason::Idle)
    );
}

#[test]
fn empty_but_fresh_build_does_not_trigger() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    updater.observe(Some(0), Duration::from_mins(1), t0);
    assert_eq!(
        updater.observe(
            Some(0),
            Duration::from_mins(2),
            t0 + Duration::from_hours(1)
        ),
        None
    );
}

#[test]
fn players_online_resets_the_empty_streak() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    updater.observe(Some(0), 2 * DAY, t0);
    assert_eq!(
        updater.observe(Some(2), 2 * DAY, t0 + Duration::from_mins(1)),
        None
    );
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_mins(2)),
        None
    );
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(430)),
        Some(RelaunchReason::Idle)
    );
}

#[test]
fn unknown_occupancy_preserves_the_streak_and_never_acts() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    updater.observe(Some(0), 2 * DAY, t0);
    assert_eq!(
        updater.observe(None, 2 * DAY, t0 + Duration::from_secs(200)),
        None
    );
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(301)),
        Some(RelaunchReason::Idle)
    );
}

#[test]
fn occupied_past_max_uptime_triggers_backstop() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    assert_eq!(updater.observe(Some(4), 2 * DAY, t0), None);
    assert_eq!(
        updater.observe(
            Some(4),
            Duration::from_mins(5000),
            t0 + Duration::from_mins(1)
        ),
        Some(RelaunchReason::Backstop)
    );
}

#[test]
fn note_relaunched_clears_the_streak() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    updater.observe(Some(0), 2 * DAY, t0);
    updater.note_relaunched();
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(301)),
        None
    );
}
