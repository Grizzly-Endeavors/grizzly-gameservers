use super::*;
use crate::agones::ServerState;

fn summary(name: &str, game: Option<&str>, state: &str, address: Option<&str>) -> ServerSummary {
    ServerSummary {
        name: name.to_owned(),
        game: game.map(str::to_owned),
        state: ServerState::from_agones(state),
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
            Some("survival.gameservers.grizzly-endeavors.com:7000"),
        ),
        summary(
            "valheim",
            Some("valheim"),
            "Allocated",
            Some("valheim.gameservers.grizzly-endeavors.com:7001"),
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
            && first.contains("survival.gameservers.grizzly-endeavors.com:7000"),
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
fn list_with_only_an_allocated_server_is_green() {
    let servers = [summary("survival", Some("minecraft"), "Allocated", None)];
    assert_eq!(
        server_list_spec(&servers).colour,
        COLOUR_UP,
        "a claimed (Allocated) server is at least as up as Ready, so it should also colour green"
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
        shutdown_spec(&ShutdownOutcome::NotFound, "ghost").colour,
        COLOUR_ERROR,
        "shutting down a nonexistent server is an error"
    );
    assert_eq!(
        destroy_spec(&DestroyOutcome::NotFound, "ghost").colour,
        COLOUR_ERROR,
        "destroying a nonexistent server is an error"
    );
}

#[test]
fn shutdown_and_destroy_success_stay_neutral() {
    assert_eq!(
        shutdown_spec(&ShutdownOutcome::Down, "minecraft").colour,
        COLOUR_NEUTRAL,
        "a clean shutdown is a no-drama neutral state"
    );
    assert_eq!(
        destroy_spec(&DestroyOutcome::Destroyed, "minecraft").colour,
        COLOUR_NEUTRAL,
        "a confirmed destruction is a no-drama neutral state"
    );
}

#[test]
fn not_managed_outcomes_explain_the_boundary() {
    let spec = shutdown_spec(&ShutdownOutcome::NotManaged, "platform-thing");
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

#[test]
fn supervisor_rejection_is_an_error_naming_the_reason() {
    let spec = supervisor_spec(
        &SupervisorOutcome::Failed("world is mid-save, try again shortly".to_owned()),
        "survival",
    );
    assert_eq!(
        spec.colour, COLOUR_ERROR,
        "a refused command is an actionable error, not a transport failure"
    );
    assert!(
        spec.body.contains("world is mid-save, try again shortly"),
        "the supervisor's reason should reach the friend, got: {}",
        spec.body
    );
}

#[test]
fn join_within_embed_limit_keeps_short_lists_intact() {
    let lines = vec!["one".to_owned(), "two".to_owned(), "three".to_owned()];
    assert_eq!(join_within_embed_limit(&lines), "one\ntwo\nthree");
}

#[test]
fn join_within_embed_limit_clips_overflow_with_a_tail() {
    // 400 lines of ~40 bytes each = ~16k bytes, well past the 4096 cap.
    let lines: Vec<String> = (0..400)
        .map(|i| format!("• line number {i:04} here"))
        .collect();
    let joined = join_within_embed_limit(&lines);

    assert!(
        joined.len() <= EMBED_DESCRIPTION_LIMIT,
        "clipped body must stay within Discord's limit, got {} bytes",
        joined.len()
    );
    assert!(
        joined.contains("…and "),
        "a clipped list should tell the friend how many were omitted, got tail: {:?}",
        joined.rsplit('\n').next()
    );
    // The omitted count must be accurate: shown lines + omitted = total.
    let tail = joined.rsplit('\n').next().unwrap();
    let omitted: usize = tail
        .trim_start_matches("…and ")
        .trim_end_matches(" more")
        .parse()
        .unwrap();
    let shown = joined.lines().count() - 1; // minus the tail line
    assert_eq!(
        shown + omitted,
        lines.len(),
        "shown + omitted should equal total"
    );
}

#[test]
fn join_within_embed_limit_handles_a_single_oversized_line() {
    let lines = vec!["x".repeat(EMBED_DESCRIPTION_LIMIT + 100)];
    let joined = join_within_embed_limit(&lines);
    assert!(joined.len() <= EMBED_DESCRIPTION_LIMIT);
    assert!(joined.contains("…and 1 more"));
}

#[test]
fn restore_with_a_safety_backup_keeps_the_clean_message() {
    let spec = restore_spec(
        &RestoreOutcome::Restored {
            boot: BootState::Ready,
            safety_backup: SafetyBackup::Taken,
        },
        "survival",
    );
    assert_eq!(spec.colour, COLOUR_UP, "a healthy restore is still green");
    assert!(
        !spec.body.contains("can't be brought back"),
        "when an undo point exists the restore must not warn about losing one, got: {}",
        spec.body
    );
}

#[test]
fn restore_without_a_safety_backup_warns_theres_no_undo() {
    // Every boot state must carry the caveat — the overwrite already happened.
    for boot in [
        BootState::Ready,
        BootState::TimedOut,
        BootState::Crashed,
        BootState::Stopped,
    ] {
        let spec = restore_spec(
            &RestoreOutcome::Restored {
                boot,
                safety_backup: SafetyBackup::Absent,
            },
            "survival",
        );
        assert!(
            spec.body.contains("can't be brought back"),
            "a restore with no safety backup must tell the friend the old world is gone, got: {}",
            spec.body
        );
    }
}

#[test]
fn archived_but_storage_not_freed_is_amber_and_recoverable() {
    let spec = archive_spec(&ArchiveOutcome::ArchivedNotReleased {
        name: "survival".to_owned(),
        size_bytes: 5 * 1024 * 1024,
    });
    assert_eq!(
        spec.colour, COLOUR_PENDING,
        "a durable-but-not-released archive is a partial success, not a failure or clean success"
    );
    assert!(
        spec.body.contains("/recover"),
        "the friend must learn the archive is recoverable, got: {}",
        spec.body
    );
    assert!(
        !spec.body.to_lowercase().contains("nothing was"),
        "it must not reproduce the 'nothing was released' failure wording, got: {}",
        spec.body
    );
}
