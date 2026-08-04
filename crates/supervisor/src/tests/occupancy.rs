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

/// Pinned against a live `TShock` 6.1.0 `/v2/server/status` response. Note
/// `status` is a *string* while `playercount` is a number — modelling the former
/// as an integer would fail every parse and report occupancy as unknown forever.
#[test]
fn a_tshock_status_yields_the_player_count() {
    let body = serde_json::json!({
        "status": "200",
        "name": "",
        "serverversion": "v1.4.5.6",
        "tshockversion": "6.1.0.0",
        "port": 7777,
        "playercount": 3,
        "maxplayers": 8,
        "world": "Grizzly",
        "uptime": "0.00:00:10",
        "serverpassword": false
    });
    let parsed: TshockStatus = serde_json::from_value(body).unwrap();
    assert_eq!(parsed.playercount, 3);
}

#[test]
fn an_empty_tshock_server_parses_as_zero() {
    let body = serde_json::json!({ "status": "200", "playercount": 0 });
    let parsed: TshockStatus = serde_json::from_value(body).unwrap();
    assert_eq!(parsed.playercount, 0);
}

/// Same rule as the Satisfactory side: a response without a count must not
/// degrade to "empty", or the auto-updater restarts an occupied server.
#[test]
fn a_tshock_response_without_a_count_does_not_parse_as_empty() {
    let body = serde_json::json!({ "status": "403", "error": "not authorized" });
    assert!(serde_json::from_value::<TshockStatus>(body).is_err());
}

#[test]
fn the_status_url_is_built_from_the_base_regardless_of_trailing_slash() {
    for base in ["http://127.0.0.1:7878", "http://127.0.0.1:7878/"] {
        let api = TshockApi::new(base).unwrap();
        assert_eq!(api.status_url, "http://127.0.0.1:7878/v2/server/status");
    }
}

#[test]
fn an_occupancy_api_is_selected_by_name() {
    assert_eq!(OccupancyApi::parse("tshock"), Some(OccupancyApi::Tshock));
    assert_eq!(
        OccupancyApi::parse(" Satisfactory "),
        Some(OccupancyApi::Satisfactory)
    );
    // A typo must not silently pick a dialect — the config layer turns None into
    // a startup error rather than guessing.
    assert_eq!(OccupancyApi::parse("tshcok"), None);
    assert_eq!(OccupancyApi::parse(""), None);
}
