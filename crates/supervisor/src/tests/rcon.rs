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
fn debug_redacts_the_password() {
    let runtime = RconRuntime::new(25575, true).unwrap();
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
