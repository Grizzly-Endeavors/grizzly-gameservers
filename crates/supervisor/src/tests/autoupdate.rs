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
    // First empty reading opens the streak but grace hasn't elapsed yet.
    assert_eq!(updater.observe(Some(0), 2 * DAY, t0), None);
    // Still within grace.
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(299)),
        None
    );
    // Past grace and past the update interval -> idle relaunch.
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
    // Long past grace, but the build is only minutes old — nothing to update to.
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
    // Someone joins, clearing the streak.
    assert_eq!(
        updater.observe(Some(2), 2 * DAY, t0 + Duration::from_mins(1)),
        None
    );
    // They leave; the grace clock starts over from here, so an immediately-later
    // empty reading must not fire even though absolute time is past the original.
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
    // A console blip mid-streak must not act and must not reset the clock.
    assert_eq!(
        updater.observe(None, 2 * DAY, t0 + Duration::from_secs(200)),
        None
    );
    // The next empty reading, past grace measured from the ORIGINAL empty_since,
    // still fires — the blip didn't restart the grace clock.
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(301)),
        Some(RelaunchReason::Idle)
    );
}

#[test]
fn occupied_past_max_uptime_triggers_backstop() {
    let mut updater = AutoUpdater::new(policy());
    let t0 = Instant::now();
    // Busy server, build younger than the cap -> leave it alone.
    assert_eq!(updater.observe(Some(4), 2 * DAY, t0), None);
    // Build past the 3-day cap while still busy -> warn-then-bounce backstop.
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
    // Streak cleared: grace restarts from the next empty reading.
    assert_eq!(
        updater.observe(Some(0), 2 * DAY, t0 + Duration::from_secs(301)),
        None
    );
}
