use super::*;

#[test]
fn parses_minecraft_player_chat() {
    let line = "[12:34:56] [Server thread/INFO]: <Steve> hello @Gary how do I sleep?";
    assert_eq!(
        parse_chat_line(ChatFormat::Minecraft, line),
        Some(ChatLine {
            player: "Steve".to_owned(),
            body: "hello @Gary how do I sleep?".to_owned(),
        })
    );
}

#[test]
fn parses_minecraft_chat_with_not_secure_marker() {
    let line = "[12:34:56] [Server thread/INFO]: [Not Secure] <Alex> @Gary status?";
    assert_eq!(
        parse_chat_line(ChatFormat::Minecraft, line),
        Some(ChatLine {
            player: "Alex".to_owned(),
            body: "@Gary status?".to_owned(),
        })
    );
}

#[test]
fn ignores_non_chat_server_lines() {
    for line in [
        "[12:34:56] [Server thread/INFO]: Starting minecraft server version 1.21",
        "[12:34:56] [Server thread/INFO]: Steve joined the game",
        "[12:34:56] [Server thread/INFO]: Done (5.123s)! For help, type \"help\"",
        "not a log line at all",
    ] {
        assert_eq!(
            parse_chat_line(ChatFormat::Minecraft, line),
            None,
            "server output must not parse as chat: {line:?}"
        );
    }
}

#[test]
fn does_not_re_trigger_on_agent_tellraw_reply() {
    // The agent replies via `tellraw @a`, which renders as plain text, never in the
    // `<player> msg` shape — so its own answer can't feed the watcher another turn.
    let reply = "[12:35:00] [Server thread/INFO]: [Gary] you can sleep to skip night";
    assert_eq!(parse_chat_line(ChatFormat::Minecraft, reply), None);
    // An RCON echo (were op-broadcast ever on) is likewise not `<player>` chat.
    let rcon_echo = "[12:35:00] [Server thread/INFO]: [Rcon] tellraw @a {\"text\":\"hi\"}";
    assert_eq!(parse_chat_line(ChatFormat::Minecraft, rcon_echo), None);
}

#[test]
fn ignores_empty_player_name() {
    let line = "[12:34:56] [Server thread/INFO]: <> orphaned bracket";
    assert_eq!(parse_chat_line(ChatFormat::Minecraft, line), None);
}

#[test]
fn strips_trigger_case_insensitively() {
    assert_eq!(
        strip_trigger("@Gary", "hey @gary help me build a nether portal"),
        Some("hey help me build a nether portal".to_owned())
    );
    assert_eq!(
        strip_trigger("@Gary", "@GARY what's the difficulty?"),
        Some("what's the difficulty?".to_owned())
    );
}

#[test]
fn returns_none_when_trigger_absent() {
    assert_eq!(
        strip_trigger("@Gary", "just regular chat about diamonds"),
        None
    );
}

#[test]
fn preserves_empty_remainder_for_bare_trigger() {
    assert_eq!(strip_trigger("@Gary", "@Gary"), Some(String::new()));
    assert_eq!(strip_trigger("@Gary", "  @gary  "), Some(String::new()));
}

#[test]
fn cooldown_blocks_repeat_from_same_player_but_not_others() {
    let mut last = HashMap::new();
    assert!(
        !is_cooling_down(&mut last, "Steve"),
        "first trigger from a player is accepted"
    );
    assert!(
        is_cooling_down(&mut last, "Steve"),
        "an immediate repeat from the same player is blocked"
    );
    assert!(
        !is_cooling_down(&mut last, "Alex"),
        "a different player is unaffected by Steve's cooldown"
    );
}

#[test]
fn chat_format_parses_known_values_case_insensitively() {
    assert_eq!(
        "minecraft".parse::<ChatFormat>().unwrap(),
        ChatFormat::Minecraft
    );
    assert_eq!(
        "Minecraft".parse::<ChatFormat>().unwrap(),
        ChatFormat::Minecraft
    );
    assert!("quake".parse::<ChatFormat>().is_err());
}
