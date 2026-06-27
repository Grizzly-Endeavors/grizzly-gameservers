use super::*;

#[test]
fn summarize_builds_connection_address_from_node_port() {
    let summary = summarize(
        "survival",
        Some("minecraft"),
        Some("Ready"),
        Some(7000),
        "gameservers.bearflinn.com",
    );

    assert_eq!(summary.name, "survival", "name should pass through");
    assert_eq!(
        summary.game.as_deref(),
        Some("minecraft"),
        "game should pass through"
    );
    assert_eq!(summary.state, "Ready", "state should pass through");
    assert_eq!(
        summary.address.as_deref(),
        Some("survival.gameservers.bearflinn.com:7000"),
        "address should combine name, domain, and node port"
    );
}

#[test]
fn summarize_omits_address_when_no_node_port() {
    let summary = summarize(
        "valheim",
        Some("valheim"),
        Some("Scheduled"),
        None,
        "gameservers.bearflinn.com",
    );

    assert_eq!(
        summary.address, None,
        "address should be absent without a node port"
    );
}

#[test]
fn summarize_defaults_unknown_state_and_tolerates_missing_game() {
    let summary = summarize(
        "minecraft",
        None,
        None,
        Some(7001),
        "gameservers.bearflinn.com",
    );

    assert_eq!(
        summary.state, "Unknown",
        "missing state should render as Unknown"
    );
    assert_eq!(
        summary.game, None,
        "a missing game label should be tolerated"
    );
}
