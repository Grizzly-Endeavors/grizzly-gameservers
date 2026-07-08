use super::*;

#[test]
fn ingame_tools_are_read_only_lookups() {
    let tools = ingame_tools();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool.function.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![LIST_SERVERS, SERVER_STATUS],
        "in-game gets exactly the two read-only lookups, nothing mutating"
    );
}

#[test]
fn session_key_separates_players_and_channels() {
    let steve = session_key("123", "Steve");
    let alex = session_key("123", "Alex");
    let other_channel = session_key("456", "Steve");
    assert_ne!(steve, alex, "different players get different sessions");
    assert_ne!(
        steve, other_channel,
        "same player in another channel is separate"
    );
    assert_eq!(
        steve.0, 123,
        "a numeric channel id is used directly as the channel key"
    );
    assert_eq!(session_key("123", "Steve"), steve, "the key is stable");
}

#[test]
fn session_key_hashes_non_numeric_channel() {
    // A DM/odd channel id that doesn't parse still yields a stable key.
    let key = session_key("dm-channel", "Steve");
    assert_eq!(session_key("dm-channel", "Steve"), key);
}

#[test]
fn framed_question_marks_input_as_a_player_quote() {
    let framed = framed_question("Steve", "how do I sleep?");
    assert!(
        framed.contains("Steve"),
        "attributes the question to the player"
    );
    assert!(framed.contains("how do I sleep?"), "carries the question");
}

#[test]
fn framed_question_handles_a_bare_ping() {
    let framed = framed_question("Steve", "   ");
    assert!(
        framed.to_lowercase().contains("ask what they need"),
        "a bare @Gary prompts Gary to ask what they want, got: {framed}"
    );
}

#[test]
fn truncate_caps_long_replies() {
    let long = "a".repeat(1000);
    let capped = truncate(&long, 600);
    assert_eq!(
        capped.chars().count(),
        600,
        "capped to the limit including the ellipsis"
    );
    assert!(capped.ends_with('…'));
    let short = "brief";
    assert_eq!(
        truncate(short, 600),
        short,
        "short replies pass through unchanged"
    );
}

#[test]
fn format_server_list_is_terse() {
    assert_eq!(
        format_server_list(&[]),
        "no game servers are running right now"
    );
    let servers = vec![
        ServerSummary {
            name: "mc-one".to_owned(),
            game: Some("minecraft".to_owned()),
            state: "Ready".to_owned(),
            address: Some("mc-one.example.com:7000".to_owned()),
        },
        ServerSummary {
            name: "mc-two".to_owned(),
            game: None,
            state: "Paused".to_owned(),
            address: None,
        },
    ];
    let rendered = format_server_list(&servers);
    assert!(rendered.contains("mc-one"));
    assert!(rendered.contains("mc-two"));
    assert!(
        rendered.contains("unknown game"),
        "missing game is labelled"
    );
    assert!(
        rendered.contains("no address yet"),
        "missing address is labelled"
    );
}

#[test]
fn no_such_names_the_missing_server() {
    assert!(no_such("mc-ghost").contains("mc-ghost"));
}

#[test]
fn ingame_prompt_hardens_against_injection_and_scopes_read_only() {
    let prompt = build_ingame_system_prompt("minecraft, valheim");
    assert!(
        prompt.contains("minecraft, valheim"),
        "lists the catalog games"
    );
    let lowered = prompt.to_lowercase();
    assert!(
        lowered.contains("untrusted"),
        "flags player input as untrusted"
    );
    assert!(
        lowered.contains("admin has to do that") || lowered.contains("an admin"),
        "directs mutating requests to an admin"
    );
    assert!(lowered.contains("in-game chat"), "sets the in-game context");
}

#[test]
fn name_arg_parses_server_name() {
    let arg: NameArg = serde_json::from_str(r#"{"name":"mc-one"}"#).unwrap();
    assert_eq!(arg.name, "mc-one");
}
