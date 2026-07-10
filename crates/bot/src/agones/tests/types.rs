use super::*;

#[test]
fn from_agones_maps_the_online_states_to_their_variants() {
    assert_eq!(ServerState::from_agones("Ready"), ServerState::Ready);
    assert_eq!(
        ServerState::from_agones("Allocated"),
        ServerState::Allocated
    );
}

#[test]
fn from_agones_preserves_an_unmodelled_state_verbatim() {
    assert_eq!(
        ServerState::from_agones("Scheduled"),
        ServerState::Other("Scheduled".to_owned()),
        "an Agones state the bot doesn't reason about is carried in Other"
    );
    assert_eq!(
        ServerState::from_agones("Unhealthy").as_str(),
        "Unhealthy",
        "Other round-trips the original string for display"
    );
}

#[test]
fn is_online_is_true_only_for_ready_and_allocated() {
    assert!(ServerState::Ready.is_online());
    assert!(ServerState::Allocated.is_online());
    assert!(!ServerState::Paused.is_online());
    assert!(!ServerState::Stopped.is_online());
    assert!(!ServerState::Unknown.is_online());
    assert!(!ServerState::from_agones("Scheduled").is_online());
}

#[test]
fn as_str_and_display_agree_for_every_variant() {
    for state in [
        ServerState::Ready,
        ServerState::Allocated,
        ServerState::Paused,
        ServerState::Stopped,
        ServerState::Unknown,
        ServerState::Other("Error".to_owned()),
    ] {
        assert_eq!(state.to_string(), state.as_str());
    }
    assert_eq!(ServerState::Stopped.as_str(), "Stopped");
    assert_eq!(ServerState::Unknown.as_str(), "Unknown");
}

#[test]
fn summarize_builds_connection_address_from_node_port() {
    let summary = summarize(
        "survival",
        Some("minecraft"),
        ServerState::Ready,
        Some(7000),
        "gameservers.grizzly-endeavors.com",
    );

    assert_eq!(summary.name, "survival", "name should pass through");
    assert_eq!(
        summary.game.as_deref(),
        Some("minecraft"),
        "game should pass through"
    );
    assert_eq!(
        summary.state,
        ServerState::Ready,
        "state should pass through"
    );
    assert_eq!(
        summary.address.as_deref(),
        Some("survival.gameservers.grizzly-endeavors.com:7000"),
        "address should combine name, domain, and node port"
    );
}

#[test]
fn summarize_omits_address_when_no_node_port() {
    let summary = summarize(
        "valheim",
        Some("valheim"),
        ServerState::from_agones("Scheduled"),
        None,
        "gameservers.grizzly-endeavors.com",
    );

    assert_eq!(
        summary.address, None,
        "address should be absent without a node port"
    );
}

#[test]
fn summarize_carries_state_and_tolerates_missing_game() {
    let summary = summarize(
        "minecraft",
        None,
        ServerState::Unknown,
        Some(7001),
        "gameservers.grizzly-endeavors.com",
    );

    assert_eq!(
        summary.state,
        ServerState::Unknown,
        "the caller's parsed state should pass through unchanged"
    );
    assert_eq!(
        summary.game, None,
        "a missing game label should be tolerated"
    );
}
