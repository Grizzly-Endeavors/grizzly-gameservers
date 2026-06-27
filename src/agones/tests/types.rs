use super::*;

#[test]
fn summarize_builds_connection_address_from_node_port() {
    let summary = summarize(
        "minecraft",
        Some("Ready"),
        Some(7000),
        "gameservers.bearflinn.com",
    );

    assert_eq!(summary.name, "minecraft", "name should pass through");
    assert_eq!(summary.state, "Ready", "state should pass through");
    assert_eq!(
        summary.address.as_deref(),
        Some("minecraft.gameservers.bearflinn.com:7000"),
        "address should combine name, domain, and node port"
    );
}

#[test]
fn summarize_omits_address_when_no_node_port() {
    let summary = summarize(
        "valheim",
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
fn summarize_defaults_unknown_state() {
    let summary = summarize("minecraft", None, Some(7001), "gameservers.bearflinn.com");

    assert_eq!(
        summary.state, "Unknown",
        "missing state should render as Unknown"
    );
}
