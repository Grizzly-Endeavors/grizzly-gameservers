use super::*;

/// The response shapes are the contract with a server nothing here controls, so
/// they're pinned against the documented payloads rather than left to be
/// discovered at runtime — a rename upstream should fail here, not silently
/// report every server as unknown-occupancy forever.
#[test]
fn a_login_response_yields_its_token() {
    let body = serde_json::json!({
        "data": { "authenticationToken": "abc123" }
    });
    let parsed: LoginEnvelope = serde_json::from_value(body).unwrap();
    assert_eq!(parsed.data.authentication_token, "abc123");
}

#[test]
fn a_state_response_yields_the_connected_count() {
    let body = serde_json::json!({
        "data": {
            "serverGameState": {
                "activeSessionName": "Grizzly",
                "numConnectedPlayers": 3,
                "playerLimit": 4,
                "techTier": 5,
                "isGameRunning": true
            }
        }
    });
    let parsed: StateEnvelope = serde_json::from_value(body).unwrap();
    assert_eq!(parsed.data.server_game_state.num_connected_players, 3);
}

/// An empty server is a real, actionable reading — it's what update-on-empty
/// waits for — so it must parse as `0` rather than tripping any absence path.
#[test]
fn an_empty_server_parses_as_zero_not_as_missing() {
    let body = serde_json::json!({
        "data": { "serverGameState": { "numConnectedPlayers": 0 } }
    });
    let parsed: StateEnvelope = serde_json::from_value(body).unwrap();
    assert_eq!(parsed.data.server_game_state.num_connected_players, 0);
}

/// A response missing the count must fail to parse, so the caller reports
/// unknown. Defaulting it to zero here would tell the auto-updater the server is
/// empty and let it restart one with players on it.
#[test]
fn a_response_without_a_count_does_not_parse_as_empty() {
    let body = serde_json::json!({
        "data": { "serverGameState": { "isGameRunning": true } }
    });
    assert!(serde_json::from_value::<StateEnvelope>(body).is_err());
}

#[test]
fn an_error_envelope_does_not_parse_as_state() {
    let body = serde_json::json!({
        "errorCode": "unauthorized",
        "errorMessage": "token expired"
    });
    assert!(serde_json::from_value::<StateEnvelope>(body).is_err());
}
