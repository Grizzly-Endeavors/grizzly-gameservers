use super::*;

fn summary(name: &str, game: Option<&str>, state: &str, address: Option<&str>) -> ServerSummary {
    ServerSummary {
        name: name.to_owned(),
        game: game.map(str::to_owned),
        state: state.to_owned(),
        address: address.map(str::to_owned),
    }
}

#[test]
fn empty_list_renders_friendly_message_and_neutral_colour() {
    let spec = server_list_spec(&[]);
    assert_eq!(
        spec.body, EMPTY_MESSAGE,
        "empty list should explain nothing is running"
    );
    assert_eq!(
        spec.colour, COLOUR_NEUTRAL,
        "an empty list is not an alarming state"
    );
}

#[test]
fn populated_list_renders_one_line_per_server_with_address() {
    let servers = [
        summary(
            "survival",
            Some("minecraft"),
            "Ready",
            Some("survival.gameservers.bearflinn.com:7000"),
        ),
        summary(
            "valheim",
            Some("valheim"),
            "Allocated",
            Some("valheim.gameservers.bearflinn.com:7001"),
        ),
    ];
    let spec = server_list_spec(&servers);

    let lines: Vec<&str> = spec.body.lines().collect();
    assert_eq!(lines.len(), 2, "one line per server expected");
    let first = lines.first().copied().unwrap();
    assert!(
        first.contains("survival")
            && first.contains("minecraft")
            && first.contains("Ready")
            && first.contains("survival.gameservers.bearflinn.com:7000"),
        "first line should show the world name, its game, state, and address, got: {first}"
    );
}

#[test]
fn list_with_a_ready_server_is_green() {
    let servers = [summary(
        "survival",
        Some("minecraft"),
        "Ready",
        Some("mc:7000"),
    )];
    assert_eq!(
        server_list_spec(&servers).colour,
        COLOUR_UP,
        "a ready server should colour the list green"
    );
}

#[test]
fn list_with_no_ready_servers_stays_neutral() {
    let servers = [summary("survival", Some("minecraft"), "Scheduled", None)];
    let spec = server_list_spec(&servers);
    assert_eq!(
        spec.colour, COLOUR_NEUTRAL,
        "nothing ready yet should not read as up"
    );
    assert!(
        spec.body.contains(NO_ADDRESS),
        "missing address should render a placeholder, got: {}",
        spec.body
    );
}

#[test]
fn server_without_a_game_label_still_lists() {
    let servers = [summary("orphan", None, "Ready", Some("orphan:7000"))];
    let spec = server_list_spec(&servers);
    assert!(
        spec.body.contains("orphan") && !spec.body.contains(" · "),
        "a server with no game label should render without the game separator, got: {}",
        spec.body
    );
}

#[test]
fn ready_create_is_green_and_shows_address() {
    let outcome = CreateOutcome::Created {
        address: "minecraft.example.com:7000".to_owned(),
        ready: true,
    };
    let spec = create_spec(&outcome, "minecraft");
    assert_eq!(spec.colour, COLOUR_UP, "a ready server should be green");
    assert!(
        spec.body.contains("minecraft.example.com:7000"),
        "a ready create should surface the connect address, got: {}",
        spec.body
    );
}

#[test]
fn pending_create_is_amber() {
    let outcome = CreateOutcome::Created {
        address: "minecraft.example.com:7000".to_owned(),
        ready: false,
    };
    assert_eq!(
        create_spec(&outcome, "minecraft").colour,
        COLOUR_PENDING,
        "a server still coming up should be amber"
    );
}

#[test]
fn ports_exhausted_is_an_error() {
    assert_eq!(
        create_spec(&CreateOutcome::PortsExhausted, "minecraft").colour,
        COLOUR_ERROR,
        "running out of slots is a failure the user must act on"
    );
}

#[test]
fn unknown_game_on_start_is_an_error_naming_the_game() {
    let outcome = StartOutcome::UnknownGame("doom".to_owned());
    let spec = start_spec(&outcome, "doom-old");
    assert_eq!(spec.colour, COLOUR_ERROR, "a missing game is an error");
    assert!(
        spec.body.contains("doom"),
        "the message should name the missing game, got: {}",
        spec.body
    );
}

#[test]
fn not_found_outcomes_are_errors() {
    assert_eq!(
        kill_spec(&KillOutcome::NotFound, "ghost").colour,
        COLOUR_ERROR,
        "killing a nonexistent server is an error"
    );
    assert_eq!(
        remove_spec(&RemoveOutcome::NotFound, "ghost").colour,
        COLOUR_ERROR,
        "removing a nonexistent server is an error"
    );
}

#[test]
fn kill_and_remove_success_stay_neutral() {
    assert_eq!(
        kill_spec(&KillOutcome::Killed, "minecraft").colour,
        COLOUR_NEUTRAL,
        "a clean shutdown is a no-drama neutral state"
    );
    assert_eq!(
        remove_spec(&RemoveOutcome::Removed, "minecraft").colour,
        COLOUR_NEUTRAL,
        "a confirmed removal is a no-drama neutral state"
    );
}

#[test]
fn not_managed_outcomes_explain_the_boundary() {
    let spec = kill_spec(&KillOutcome::NotManaged, "platform-thing");
    assert_eq!(spec.colour, COLOUR_ERROR, "a refused op is an error");
    assert!(
        spec.body.contains("platform-thing"),
        "the message should name the server, got: {}",
        spec.body
    );
}

#[test]
fn paused_is_neutral_and_names_the_server() {
    let spec = supervisor_spec(&SupervisorOutcome::Paused, "survival");
    assert_eq!(
        spec.colour, COLOUR_NEUTRAL,
        "a pause is a calm, reversible state"
    );
    assert!(
        spec.body.contains("survival"),
        "the message should name the paused server, got: {}",
        spec.body
    );
}

#[test]
fn resume_and_restart_are_pending() {
    assert_eq!(
        supervisor_spec(&SupervisorOutcome::Resumed, "survival").colour,
        COLOUR_PENDING,
        "a resuming server is still coming up"
    );
    assert_eq!(
        supervisor_spec(&SupervisorOutcome::Restarted, "survival").colour,
        COLOUR_PENDING,
        "a restarting server is still coming up"
    );
}

#[test]
fn supervisor_failures_are_errors() {
    assert_eq!(
        supervisor_spec(&SupervisorOutcome::Unreachable, "survival").colour,
        COLOUR_ERROR,
        "an unreachable control api is an actionable error"
    );
    assert_eq!(
        supervisor_spec(&SupervisorOutcome::PodNotReady, "survival").colour,
        COLOUR_ERROR,
        "a not-ready pod is surfaced as a retryable error"
    );
}
