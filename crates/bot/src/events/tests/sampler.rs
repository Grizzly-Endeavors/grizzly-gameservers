use super::*;

use grizzly_control_api::{ProcessPhase, StatusResponse};

fn instance(game: &str, guild: &str) -> LiveInstance {
    LiveInstance {
        instance: InstanceName::new("worldly-otter"),
        game: GameId::new(game),
        guild: GuildId::new(guild),
        running: true,
        unhealthy: false,
    }
}

fn status(restarts: u32) -> StatusPoll {
    StatusPoll::Ok(StatusResponse {
        process: ProcessPhase::Running,
        ready: true,
        pid: Some(42),
        uptime_seconds: 900,
        restarts,
    })
}

#[test]
fn a_live_reading_records_the_count_and_the_restarts() {
    let sample = build_sample(
        &instance("minecraft", "1234"),
        &status(2),
        &FsOutcome::Ok(Some(3)),
    )
    .expect("a readable status yields a sample");

    assert_eq!(sample.instance.as_str(), "worldly-otter");
    assert_eq!(sample.game, Some(GameId::new("minecraft")));
    assert_eq!(sample.guild, Some(GuildId::new("1234")));
    assert_eq!(sample.players, Some(3));
    assert_eq!(sample.restarts, 2);
}

#[test]
fn an_unknown_count_is_never_recorded_as_empty() {
    // The load-bearing invariant: a game whose occupancy probe doesn't work
    // reports None, and None must reach the column as NULL — reading it as zero
    // would manufacture idle time that never happened.
    let sample = build_sample(
        &instance("satisfactory", "1234"),
        &status(0),
        &FsOutcome::Ok(None),
    )
    .expect("a readable status yields a sample");

    assert_eq!(sample.players, None);
}

#[test]
fn an_unreadable_occupancy_still_records_the_row() {
    // The status answered, so the instance is demonstrably up: keep the row (the
    // restart counter is still worth having) with the count unknown.
    for occupancy in [
        FsOutcome::NotFound,
        FsOutcome::NotManaged,
        FsOutcome::PodNotReady,
        FsOutcome::Unreachable,
        FsOutcome::Rejected("no rcon".to_owned()),
    ] {
        let sample = build_sample(&instance("valheim", "1234"), &status(7), &occupancy)
            .expect("a readable status yields a sample");
        assert_eq!(sample.players, None);
        assert_eq!(sample.restarts, 7);
    }
}

#[test]
fn an_unreadable_status_records_nothing() {
    // Without the restart counter there is no sample, and every one of these
    // describes an instance mid-transition rather than an idle one.
    for poll in [
        StatusPoll::NotFound,
        StatusPoll::NotManaged,
        StatusPoll::PodNotReady,
        StatusPoll::Unreachable,
    ] {
        assert!(
            build_sample(
                &instance("minecraft", "1234"),
                &poll,
                &FsOutcome::Ok(Some(1))
            )
            .is_none()
        );
    }
}

#[test]
fn poll_shapes_map_one_to_one_for_the_watchdog() {
    assert_eq!(
        poll_of(&status(3)),
        PollObservation::Ok {
            phase: ProcessPhase::Running,
            restarts: 3,
            uptime_seconds: 900,
        }
    );
    assert_eq!(
        poll_of(&StatusPoll::Unreachable),
        PollObservation::Unreachable
    );
    assert_eq!(
        poll_of(&StatusPoll::PodNotReady),
        PollObservation::PodNotReady
    );
    assert_eq!(poll_of(&StatusPoll::NotFound), PollObservation::NotFound);
    assert_eq!(
        poll_of(&StatusPoll::NotManaged),
        PollObservation::NotManaged
    );
}

#[test]
fn observations_carry_labels_with_the_same_null_semantics_as_rows() {
    let observed = observation(&instance("", ""), PollObservation::GameServerUnhealthy);
    assert_eq!(observed.instance.as_str(), "worldly-otter");
    assert_eq!(
        observed.game, None,
        "an absent label must not read as a blank game"
    );
    assert_eq!(observed.guild, None);
    assert_eq!(observed.poll, PollObservation::GameServerUnhealthy);
}

#[test]
fn an_absent_label_lands_as_null_not_a_blank_string() {
    // The lister defaults a missing label to "", which in the column would read
    // as a real game or guild whose name happens to be empty.
    let sample = build_sample(&instance("", ""), &status(0), &FsOutcome::Ok(Some(0)))
        .expect("a readable status yields a sample");

    assert_eq!(sample.game, None);
    assert_eq!(sample.guild, None);
    // A *measured* zero is a real reading and stays zero — only unknowns are null.
    assert_eq!(sample.players, Some(0));
}
