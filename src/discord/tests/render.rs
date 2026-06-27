#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use super::*;

fn summary(name: &str, state: &str, address: Option<&str>) -> ServerSummary {
    ServerSummary {
        name: name.to_owned(),
        state: state.to_owned(),
        address: address.map(str::to_owned),
    }
}

#[test]
fn empty_list_renders_friendly_message() {
    let rendered = format_server_list(&[]);
    assert_eq!(
        rendered, "No game servers are running right now.",
        "empty list should explain nothing is running"
    );
}

#[test]
fn populated_list_renders_one_line_per_server() {
    let servers = [
        summary(
            "minecraft",
            "Ready",
            Some("minecraft.gameservers.bearflinn.com:7000"),
        ),
        summary(
            "valheim",
            "Allocated",
            Some("valheim.gameservers.bearflinn.com:7001"),
        ),
    ];
    let rendered = format_server_list(&servers);

    let lines: Vec<&str> = rendered.lines().collect();
    assert_eq!(lines.len(), 2, "one line per server expected");
    let first = lines.first().copied().unwrap();
    assert!(
        first.contains("minecraft") && first.contains("Ready"),
        "first line should describe minecraft, got: {first}"
    );
    assert!(
        first.contains("minecraft.gameservers.bearflinn.com:7000"),
        "first line should include the connection address, got: {first}"
    );
}

#[test]
fn server_without_address_shows_placeholder() {
    let servers = [summary("minecraft", "Scheduled", None)];
    let rendered = format_server_list(&servers);
    assert!(
        rendered.contains("(not exposed yet)"),
        "missing address should render a placeholder, got: {rendered}"
    );
}
