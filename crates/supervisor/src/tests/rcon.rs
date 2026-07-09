use super::*;

#[test]
fn generate_password_is_hex_of_expected_length() {
    let password = generate_password().unwrap();
    assert_eq!(
        password.len(),
        PASSWORD_BYTES * 2,
        "password should be a hex encoding of {PASSWORD_BYTES} bytes"
    );
    assert!(
        password.chars().all(|c| c.is_ascii_hexdigit()),
        "password should be all hex digits, got {password:?}"
    );
}

#[test]
fn generate_password_is_not_constant() {
    let a = generate_password().unwrap();
    let b = generate_password().unwrap();
    assert_ne!(a, b, "two minted passwords should differ");
}

#[test]
fn new_truncates_the_password_to_the_cap() {
    let full = RconRuntime::new(25575, RconDialect::Source, None).unwrap();
    let capped = RconRuntime::new(25575, RconDialect::Source, Some(30)).unwrap();
    assert!(
        full.password().len() > 30,
        "the default password should exceed Palworld's cap"
    );
    assert_eq!(
        capped.password().len(),
        30,
        "a cap shorter than the minted length truncates it"
    );
    let generous = RconRuntime::new(25575, RconDialect::Source, Some(4096)).unwrap();
    assert_eq!(
        generous.password().len(),
        full.password().len(),
        "a cap longer than the minted length leaves it untouched"
    );
}

#[test]
fn debug_redacts_the_password() {
    let runtime = RconRuntime::new(25575, RconDialect::Minecraft, None).unwrap();
    let rendered = format!("{runtime:?}");
    assert!(
        rendered.contains("<redacted>"),
        "Debug should redact the password, got {rendered}"
    );
    assert!(
        !rendered.contains(runtime.password()),
        "Debug must not contain the real password"
    );
}

#[test]
fn minecraft_parses_online_count_from_list_reply() {
    let dialect = RconDialect::Minecraft;
    assert_eq!(
        dialect.parse_player_count("There are 3 of a max of 20 players online: a, b, c"),
        Some(3)
    );
    assert_eq!(
        dialect.parse_player_count("There are 0 of a max of 20 players online:"),
        Some(0)
    );
}

#[test]
fn source_counts_factorio_online_rows() {
    let dialect = RconDialect::Source;
    // `/players` lists everyone; only connected players carry the " (online)"
    // suffix, and offline rows must not be counted.
    let reply = "Online players (2):\n  Alice (online)\n  Bob (online)\n  Carol (offline)\n";
    assert_eq!(dialect.parse_player_count(reply), Some(2));
    // Empty server: header only, no online rows.
    assert_eq!(dialect.parse_player_count("Online players (0):\n"), Some(0));
    assert_eq!(dialect.parse_player_count(""), Some(0));
}

#[test]
fn palworld_counts_showplayers_rows() {
    let dialect = RconDialect::Palworld;
    // Header only -> empty server.
    assert_eq!(
        dialect.parse_player_count("name,playeruid,steamid"),
        Some(0)
    );
    assert_eq!(
        dialect.parse_player_count("name,playeruid,steamid\nAlice,1,7656\nBob,2,7657"),
        Some(2)
    );
    // Trailing blank lines are not players.
    assert_eq!(
        dialect.parse_player_count("name,playeruid,steamid\nAlice,1,7656\n\n"),
        Some(1)
    );
}

#[test]
fn valheim_parses_online_count_from_players_reply() {
    let dialect = RconDialect::Valheim;
    // ValheimRcon's `players` opens with "Online N", then one line per player.
    assert_eq!(dialect.parse_player_count("Online 0\n"), Some(0));
    assert_eq!(
        dialect.parse_player_count("Online 2\nAlice (10,-5) Meadows\nBob (30,12) BlackForest\n"),
        Some(2)
    );
}

#[test]
fn valheim_broadcasts_with_say() {
    let command = broadcast_command("restarting soon", RconDialect::Valheim).unwrap();
    assert_eq!(command, "say restarting soon");
}

#[test]
fn valheim_dialect_parses_from_str() {
    assert_eq!(
        "valheim".parse::<RconDialect>().unwrap(),
        RconDialect::Valheim
    );
    assert!("bogus".parse::<RconDialect>().is_err());
}

#[test]
fn player_count_command_matches_dialect() {
    assert_eq!(RconDialect::Minecraft.player_count_command(), "list");
    assert_eq!(RconDialect::Source.player_count_command(), "/players");
    assert_eq!(RconDialect::Palworld.player_count_command(), "ShowPlayers");
    assert_eq!(RconDialect::Valheim.player_count_command(), "players");
}

#[test]
fn encode_packet_frames_length_id_type_and_terminators() {
    let bytes = encode_packet(ID_EXEC, TYPE_EXECCOMMAND, "list").unwrap();
    // length field counts everything after itself: id + type + body + 2 nulls = 14.
    let mut expected = Vec::new();
    expected.extend_from_slice(&14_i32.to_le_bytes());
    expected.extend_from_slice(&ID_EXEC.to_le_bytes());
    expected.extend_from_slice(&TYPE_EXECCOMMAND.to_le_bytes());
    expected.extend_from_slice(b"list");
    expected.extend_from_slice(&[0, 0]);
    assert_eq!(bytes, expected, "framed packet layout");
}

#[tokio::test]
async fn read_packet_round_trips_an_encoded_packet() {
    let encoded = encode_packet(7, TYPE_RESPONSE_VALUE, "hello there").unwrap();
    let mut reader = encoded.as_slice();
    let packet = read_packet(&mut reader).await.unwrap();
    assert_eq!(
        packet,
        Packet {
            id: 7,
            kind: TYPE_RESPONSE_VALUE,
            body: "hello there".to_owned(),
        }
    );
}

#[tokio::test]
async fn read_packet_handles_an_empty_body() {
    let encoded = encode_packet(ID_SENTINEL, TYPE_RESPONSE_VALUE, "").unwrap();
    let mut reader = encoded.as_slice();
    let packet = read_packet(&mut reader).await.unwrap();
    assert_eq!(
        packet.body, "",
        "an empty-body packet decodes to an empty string"
    );
    assert_eq!(packet.id, ID_SENTINEL);
}

#[tokio::test]
async fn read_packet_rejects_a_too_short_length_prefix() {
    // Length prefix of 4 is below the 10-byte minimum (id + type + 2 nulls).
    let mut bytes = 4_i32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&[0_u8; 4]);
    let mut reader = bytes.as_slice();
    assert!(
        read_packet(&mut reader).await.is_err(),
        "an undersized packet length should be rejected"
    );
}

#[tokio::test]
async fn read_packet_rejects_an_oversized_length_prefix() {
    // Above MAX_PACKET_LEN; rejected before any body bytes are read, so no body
    // needs to be supplied.
    let bytes = i32::try_from(MAX_PACKET_LEN + 1)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    let mut reader = bytes.as_slice();
    assert!(
        read_packet(&mut reader).await.is_err(),
        "an oversized packet length should be rejected"
    );
}

#[tokio::test]
async fn connect_with_retry_waits_out_a_late_binding_listener() {
    // Reserve a port, then drop the listener so the address is refused, mirroring
    // the boot window where RCON hasn't bound yet.
    let probe = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = probe.local_addr().unwrap();
    drop(probe);

    // Bring the listener up after a couple of retry intervals; connect_with_retry
    // should keep polling until it binds rather than failing on the refusal.
    let binder = tokio::spawn(async move {
        sleep(CONNECT_RETRY_INTERVAL * 3).await;
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        listener.accept().await.unwrap();
    });

    let connected = timeout(RCON_TIMEOUT, connect_with_retry(address)).await;
    assert!(
        matches!(connected, Ok(Ok(_))),
        "connect should succeed once the listener binds, got {connected:?}"
    );
    binder.await.unwrap();
}

#[test]
fn broadcast_command_builds_a_minecraft_tellraw() {
    let command = broadcast_command("Gary: ran `op Bear`", RconDialect::Minecraft).unwrap();
    assert!(
        command.starts_with("tellraw @a "),
        "minecraft broadcast should use tellraw, got {command:?}"
    );
    // The message must be carried as a JSON text component (serde-escaped), not
    // hand-quoted, so it survives special characters.
    let json = command.strip_prefix("tellraw @a ").unwrap();
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(value.get("text").unwrap(), "Gary: ran `op Bear`");
}

#[test]
fn broadcast_command_escapes_message_json() {
    // A quote in the message must not break out of the JSON string.
    let command = broadcast_command(r#"Gary: said "hi""#, RconDialect::Minecraft).unwrap();
    let json = command.strip_prefix("tellraw @a ").unwrap();
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(value.get("text").unwrap(), r#"Gary: said "hi""#);
}

#[test]
fn broadcast_command_falls_back_to_say_for_source() {
    let command = broadcast_command("Gary: heads up", RconDialect::Source).unwrap();
    assert_eq!(command, "say Gary: heads up");
}

#[test]
fn broadcast_command_uses_palworld_broadcast_verb() {
    let command = broadcast_command("Gary: heads up", RconDialect::Palworld).unwrap();
    assert_eq!(command, "Broadcast Gary: heads up");
}

#[test]
fn dialect_parses_case_insensitively_and_rejects_unknown() {
    assert_eq!(
        "Minecraft".parse::<RconDialect>().unwrap(),
        RconDialect::Minecraft
    );
    assert_eq!(
        "source".parse::<RconDialect>().unwrap(),
        RconDialect::Source
    );
    assert_eq!(
        " palworld ".parse::<RconDialect>().unwrap(),
        RconDialect::Palworld
    );
    assert!("halflife".parse::<RconDialect>().is_err());
}

#[test]
fn single_packet_reply_covers_minecraft_and_palworld_only() {
    assert!(RconDialect::Minecraft.single_packet_reply());
    assert!(RconDialect::Palworld.single_packet_reply());
    assert!(
        !RconDialect::Source.single_packet_reply(),
        "a correct Source server fragments; it must use the sentinel read"
    );
}

#[test]
fn truncate_output_leaves_short_text_untouched() {
    let text = "There are 2 of a max of 20 players online".to_owned();
    assert_eq!(truncate_output(text.clone()), text);
}

#[test]
fn truncate_output_caps_long_text_at_a_char_boundary() {
    let long = "x".repeat(MAX_OUTPUT_BYTES + 100);
    let truncated = truncate_output(long);
    assert!(
        truncated.ends_with("… (truncated)"),
        "an over-cap reply should be flagged as truncated"
    );
    assert!(
        truncated.len() <= MAX_OUTPUT_BYTES + "… (truncated)".len(),
        "truncated output should be bounded by the cap plus the marker"
    );
}
