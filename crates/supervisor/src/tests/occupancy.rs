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

#[test]
fn a2s_is_selected_by_name() {
    assert_eq!(OccupancyApi::parse("a2s"), Some(OccupancyApi::A2s));
    assert_eq!(OccupancyApi::parse(" A2S "), Some(OccupancyApi::A2s));
}

/// A challenge reply as a live Enshrouded server sent it.
const ENSHROUDED_CHALLENGE: &str = "ffffffff41b99f9175";

/// An info reply as a live Enshrouded server sent it once the challenge was
/// repeated: an empty map string, no players, 16 slots. Pinned verbatim because
/// the field layout is the contract with a server nothing here controls.
const ENSHROUDED_INFO: &str = "ffffffff4911456e7368726f7564656420436f6e7461696e6572697a65640000456e7368726f7564656400456e7368726f75646564000000001000646c0000302e302e31352e3000b1153d16c426a3bfc84001313b313032343233333b303b303b313b303b303b303b3000a45d120000000000";

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn an_enshrouded_challenge_yields_its_four_bytes() {
    assert_eq!(
        parse_a2s_info_reply(&hex(ENSHROUDED_CHALLENGE)).unwrap(),
        A2sInfoReply::Challenge([0xb9, 0x9f, 0x91, 0x75])
    );
}

/// An empty server is the reading update-on-empty waits for, so it must parse as
/// `0` — and the empty map string before it must not throw the field walk off.
#[test]
fn an_empty_enshrouded_server_parses_as_zero_players() {
    assert_eq!(
        parse_a2s_info_reply(&hex(ENSHROUDED_INFO)).unwrap(),
        A2sInfoReply::Players(0)
    );
}

fn info_reply(players: u8) -> Vec<u8> {
    let mut reply = vec![0xFF, 0xFF, 0xFF, 0xFF, b'I', 0x11];
    for field in ["Grizzly", "", "Enshrouded", "Enshrouded"] {
        reply.extend_from_slice(field.as_bytes());
        reply.push(0);
    }
    reply.extend_from_slice(&[0x00, 0x00, players, 16, 0]);
    reply
}

#[test]
fn an_info_reply_yields_its_player_count() {
    assert_eq!(
        parse_a2s_info_reply(&info_reply(3)).unwrap(),
        A2sInfoReply::Players(3)
    );
}

/// A truncated reply must fail rather than read some other byte as the count —
/// a misread zero would let update-on-empty bounce a server with people on it.
#[test]
fn a_reply_that_ends_before_the_player_count_is_an_error() {
    let mut reply = info_reply(3);
    reply.truncate(reply.len() - 3);
    assert!(parse_a2s_info_reply(&reply).is_err());
}

#[test]
fn replies_that_are_not_single_packet_info_or_challenge_are_errors() {
    let split_packet = [0xFF, 0xFF, 0xFF, 0xFE, b'I', 0x11];
    let player_list = [0xFF, 0xFF, 0xFF, 0xFF, b'D', 0x00];
    let short_challenge = [0xFF, 0xFF, 0xFF, 0xFF, b'A', 0x01, 0x02];
    for reply in [
        &split_packet[..],
        &player_list[..],
        &short_challenge[..],
        &[],
    ] {
        assert!(parse_a2s_info_reply(reply).is_err(), "{reply:?}");
    }
}

/// Serve `A2S_INFO` on loopback the way Enshrouded does: challenge a bare request,
/// answer one that carries the challenge. Returns the port.
async fn spawn_challenging_a2s_server(players: u8) -> u16 {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = socket.local_addr().unwrap().port();
    tokio::spawn(async move {
        let challenge = [0xb9, 0x9f, 0x91, 0x75];
        let mut buf = [0_u8; 1400];
        loop {
            let (len, peer) = socket.recv_from(&mut buf).await.unwrap();
            let request = buf.get(..len).unwrap();
            let reply = if request == [A2S_INFO_REQUEST, &challenge].concat() {
                info_reply(players)
            } else {
                [&[0xFF, 0xFF, 0xFF, 0xFF, b'A'][..], &challenge].concat()
            };
            socket.send_to(&reply, peer).await.unwrap();
        }
    });
    port
}

#[tokio::test]
async fn the_query_answers_the_challenge_and_reads_the_count() {
    let port = spawn_challenging_a2s_server(5).await;
    assert_eq!(A2sQuery::new(port).player_count().await, Some(5));
}

/// A silent query port — the world is still loading — is unknown occupancy, never
/// an empty server.
#[tokio::test]
async fn a_silent_query_port_is_unknown_not_empty() {
    let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = silent.local_addr().unwrap().port();
    assert_eq!(A2sQuery::new(port).player_count().await, None);
}
