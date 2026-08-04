use super::*;

const INTERVAL: Duration = Duration::from_mins(5);

fn detector() -> DetectorState {
    DetectorState::new(INTERVAL)
}

fn ok(restarts: u32) -> PollObservation {
    PollObservation::Ok {
        phase: ProcessPhase::Running,
        restarts,
        uptime_seconds: 600,
    }
}

fn crashed(restarts: u32) -> PollObservation {
    PollObservation::Ok {
        phase: ProcessPhase::Crashed,
        restarts,
        uptime_seconds: 0,
    }
}

fn obs(name: &str, poll: PollObservation) -> StatusObservation {
    StatusObservation {
        instance: InstanceName::new(name),
        game: Some(GameId::new("minecraft")),
        guild: Some(GuildId::new("42")),
        poll,
    }
}

fn mem_sample(name: &str, mem_bytes: Option<i64>, mem_limit_bytes: Option<i64>) -> ResourceSample {
    ResourceSample {
        instance: InstanceName::new(name),
        game: Some(GameId::new("minecraft")),
        guild: Some(GuildId::new("42")),
        cpu_millicores: None,
        mem_bytes,
        cpu_request_millicores: None,
        mem_request_bytes: None,
        cpu_limit_millicores: None,
        mem_limit_bytes,
        pvc_bytes: None,
    }
}

/// `n` ticks past an arbitrary start, so tests can march wall-clock forward.
fn at(t0: Instant, ticks: u32) -> Instant {
    t0 + INTERVAL * ticks
}

#[test]
fn first_sighting_baselines_without_firing() {
    let mut detector = detector();
    let t0 = Instant::now();
    let fired = detector.tick(t0, &[obs("mc-a", ok(5))], &[]);
    assert!(fired.is_empty(), "a first sighting must only baseline");
}

#[test]
fn delta_of_two_in_one_tick_fires_crash_loop() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(0))], &[]);
    let fired = detector.tick(at(t0, 1), &[obs("mc-a", ok(2))], &[]);
    assert_eq!(fired.len(), 1);
    let incident = fired.first().unwrap();
    assert_eq!(incident.kind, IncidentKind::Crash);
    assert_eq!(incident.instance.as_str(), "mc-a");
    assert_eq!(incident.game, Some(GameId::new("minecraft")));
    assert_eq!(incident.guild, Some(GuildId::new("42")));
    match &incident.facts {
        IncidentFacts::CrashLoop {
            restarts_delta,
            window_minutes,
            total_restarts,
            ..
        } => {
            assert_eq!(*restarts_delta, 2);
            assert_eq!(*window_minutes, 15);
            assert_eq!(*total_restarts, 2);
        }
        other @ (IncidentFacts::Crashed { .. }
        | IncidentFacts::Unreachable { .. }
        | IncidentFacts::MemoryPressure { .. }) => {
            panic!("expected CrashLoop facts, got {other:?}")
        }
    }
}

#[test]
fn delta_split_across_ticks_fires_crash_loop() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(0))], &[]);
    assert!(
        detector
            .tick(at(t0, 1), &[obs("mc-a", ok(1))], &[])
            .is_empty()
    );
    let fired = detector.tick(at(t0, 2), &[obs("mc-a", ok(2))], &[]);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired.first().unwrap().kind, IncidentKind::Crash);
}

#[test]
fn lone_crash_stays_silent_and_ages_out_of_the_window() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(0))], &[]);
    assert!(
        detector
            .tick(at(t0, 1), &[obs("mc-a", ok(1))], &[])
            .is_empty()
    );
    // Three quiet ticks push the lone delta out of the window...
    for tick in 2..5 {
        assert!(
            detector
                .tick(at(t0, tick), &[obs("mc-a", ok(1))], &[])
                .is_empty()
        );
    }
    // ...so a later single crash still doesn't reach the threshold.
    assert!(
        detector
            .tick(at(t0, 5), &[obs("mc-a", ok(2))], &[])
            .is_empty()
    );
}

#[test]
fn counter_reset_rebaselines_silently() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(5))], &[]);
    // The pod was replaced: the counter fell to zero. No firing, ring cleared.
    assert!(
        detector
            .tick(at(t0, 1), &[obs("mc-a", ok(0))], &[])
            .is_empty()
    );
    // One crash on the fresh pod is still just one crash.
    assert!(
        detector
            .tick(at(t0, 2), &[obs("mc-a", ok(1))], &[])
            .is_empty()
    );
}

#[test]
fn crashed_phase_fires_immediately_even_on_first_sighting() {
    let mut detector = detector();
    let t0 = Instant::now();
    let fired = detector.tick(t0, &[obs("mc-a", crashed(5))], &[]);
    assert_eq!(fired.len(), 1);
    let incident = fired.first().unwrap();
    assert_eq!(incident.kind, IncidentKind::Crash);
    assert_eq!(incident.facts, IncidentFacts::Crashed { total_restarts: 5 });
}

#[test]
fn crashed_and_crash_loop_share_one_cooldown() {
    let mut detector = detector();
    let t0 = Instant::now();
    assert_eq!(detector.tick(t0, &[obs("mc-a", crashed(5))], &[]).len(), 1);
    // A crash-loop delta right after the crashed fire is the same illness.
    let fired = detector.tick(at(t0, 1), &[obs("mc-a", ok(7))], &[]);
    assert!(fired.is_empty(), "crash kinds must share a cooldown");
}

#[test]
fn unreachable_fires_at_exactly_three_consecutive_ticks() {
    let mut detector = detector();
    let t0 = Instant::now();
    assert!(
        detector
            .tick(t0, &[obs("mc-a", PollObservation::Unreachable)], &[])
            .is_empty()
    );
    assert!(
        detector
            .tick(at(t0, 1), &[obs("mc-a", PollObservation::Unreachable)], &[])
            .is_empty()
    );
    let fired = detector.tick(at(t0, 2), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    assert_eq!(fired.len(), 1);
    let incident = fired.first().unwrap();
    assert_eq!(incident.kind, IncidentKind::Wedged);
    assert_eq!(incident.facts, IncidentFacts::Unreachable { minutes: 15 });
}

#[test]
fn unreachable_streak_resets_on_a_successful_poll() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", PollObservation::Unreachable)], &[]);
    detector.tick(at(t0, 1), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    detector.tick(at(t0, 2), &[obs("mc-a", ok(0))], &[]);
    detector.tick(at(t0, 3), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    let fired = detector.tick(at(t0, 4), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    assert!(fired.is_empty(), "an ok poll must reset the streak");
}

#[test]
fn unreachable_streak_holds_through_unlearnable_ticks() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", PollObservation::Unreachable)], &[]);
    detector.tick(at(t0, 1), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    // A failed cluster query says nothing either way; the streak holds at two.
    detector.tick(at(t0, 2), &[obs("mc-a", PollObservation::ReadFailed)], &[]);
    let fired = detector.tick(at(t0, 3), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired.first().unwrap().kind, IncidentKind::Wedged);
}

#[test]
fn memory_pressure_fires_after_three_over_threshold_ticks() {
    let mut detector = detector();
    let t0 = Instant::now();
    let sample = || mem_sample("mc-a", Some(950), Some(1000));
    assert!(detector.tick(t0, &[], &[sample()]).is_empty());
    assert!(detector.tick(at(t0, 1), &[], &[sample()]).is_empty());
    let fired = detector.tick(at(t0, 2), &[], &[sample()]);
    assert_eq!(fired.len(), 1);
    let incident = fired.first().unwrap();
    assert_eq!(incident.kind, IncidentKind::Memory);
    assert_eq!(incident.game, Some(GameId::new("minecraft")));
    assert_eq!(
        incident.facts,
        IncidentFacts::MemoryPressure {
            used_bytes: 950,
            limit_bytes: 1000,
            percent: 95,
        }
    );
}

#[test]
fn memory_streak_holds_on_a_missing_reading() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    detector.tick(at(t0, 1), &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    // Metrics-server missed a beat: unknown is neither fine nor pressure.
    detector.tick(at(t0, 2), &[], &[mem_sample("mc-a", None, Some(1000))]);
    let fired = detector.tick(at(t0, 3), &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    assert_eq!(fired.len(), 1);
}

#[test]
fn memory_streak_resets_below_threshold() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    detector.tick(at(t0, 1), &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    detector.tick(at(t0, 2), &[], &[mem_sample("mc-a", Some(500), Some(1000))]);
    detector.tick(at(t0, 3), &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    let fired = detector.tick(at(t0, 4), &[], &[mem_sample("mc-a", Some(950), Some(1000))]);
    assert!(fired.is_empty(), "a healthy reading must reset the streak");
}

#[test]
fn memory_never_fires_without_a_declared_limit() {
    let mut detector = detector();
    let t0 = Instant::now();
    for tick in 0..5 {
        let fired = detector.tick(at(t0, tick), &[], &[mem_sample("mc-a", Some(950), None)]);
        assert!(fired.is_empty());
    }
}

#[test]
fn cooldown_suppresses_refires_then_permits_after_expiry() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(0))], &[]);
    assert_eq!(
        detector.tick(at(t0, 1), &[obs("mc-a", ok(2))], &[]).len(),
        1
    );
    // Still crashing ten minutes later: inside the cooldown, suppressed.
    assert!(
        detector
            .tick(at(t0, 3), &[obs("mc-a", ok(4))], &[])
            .is_empty()
    );
    // Over an hour after the fire, the same signal wakes Gary again.
    let fired = detector.tick(t0 + Duration::from_mins(70), &[obs("mc-a", ok(6))], &[]);
    assert_eq!(fired.len(), 1);
}

#[test]
fn pruning_forgets_a_departed_instance_and_its_cooldowns() {
    let mut detector = detector();
    let t0 = Instant::now();
    assert_eq!(detector.tick(t0, &[obs("mc-a", crashed(5))], &[]).len(), 1);
    // The instance vanishes from both listings (destroyed)...
    detector.tick(at(t0, 1), &[], &[]);
    // ...so a namesake created later starts clean, cooldown included.
    let fired = detector.tick(at(t0, 2), &[obs("mc-a", crashed(5))], &[]);
    assert_eq!(fired.len(), 1);
}

#[test]
fn resource_only_presence_keeps_state_alive() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", PollObservation::Unreachable)], &[]);
    detector.tick(at(t0, 1), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    // Present only in the resource listing this tick (e.g. briefly paused):
    // not pruned, and the unreachable streak holds rather than resetting.
    detector.tick(at(t0, 2), &[], &[mem_sample("mc-a", Some(10), Some(1000))]);
    let fired = detector.tick(at(t0, 3), &[obs("mc-a", PollObservation::Unreachable)], &[]);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired.first().unwrap().kind, IncidentKind::Wedged);
}

#[test]
fn incidents_fire_per_instance_independently() {
    let mut detector = detector();
    let t0 = Instant::now();
    detector.tick(t0, &[obs("mc-a", ok(0)), obs("mc-b", ok(0))], &[]);
    let fired = detector.tick(at(t0, 1), &[obs("mc-a", ok(2)), obs("mc-b", ok(0))], &[]);
    assert_eq!(fired.len(), 1);
    assert_eq!(fired.first().unwrap().instance.as_str(), "mc-a");
}
