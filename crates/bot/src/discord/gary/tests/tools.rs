use crate::agones::{FsOutcome, RemoveOutcome, ServerSummary, SupervisorOutcome};

use super::*;

fn tool_names(is_admin: bool) -> Vec<String> {
    available_tools(is_admin)
        .into_iter()
        .map(|tool| tool.function.name)
        .collect()
}

#[test]
fn non_admins_get_only_read_only_tools() {
    let names = tool_names(false);
    assert_eq!(names, vec![LIST_SERVERS, SERVER_STATUS]);
    assert!(
        !names
            .iter()
            .any(|name| name == CREATE_SERVER || name == REMOVE_SERVER),
        "read-only tier must not expose any mutating tool"
    );
}

#[test]
fn admins_get_the_full_lifecycle_and_filesystem_set() {
    let names = tool_names(true);
    for expected in [
        LIST_SERVERS,
        SERVER_STATUS,
        CREATE_SERVER,
        STOP_SERVER,
        START_SERVER,
        RESTART_SERVER,
        KILL_SERVER,
        REMOVE_SERVER,
        BROWSE_FILES,
        READ_FILE,
        READ_LOGS,
        WRITE_FILE,
        RESTORE_FILE,
        SEND_COMMAND,
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
    assert_eq!(
        names.len(),
        14,
        "eight lifecycle tools, five filesystem tools, and send_command"
    );
}

#[test]
fn filesystem_tools_are_admin_only() {
    let names = tool_names(false);
    for tool in [BROWSE_FILES, READ_FILE, READ_LOGS, WRITE_FILE, RESTORE_FILE] {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} must not be offered to non-admins"
        );
    }
}

#[test]
fn send_command_is_admin_only() {
    assert!(
        !tool_names(false).iter().any(|name| name == SEND_COMMAND),
        "send_command must not be offered to non-admins"
    );
    assert!(
        tool_names(true).iter().any(|name| name == SEND_COMMAND),
        "send_command must be offered to admins"
    );
}

#[test]
fn command_param_schema_exposes_name_and_command() {
    let schema = params_schema::<CommandParams>();
    let properties = schema
        .as_object()
        .and_then(|object| object.get("properties"))
        .and_then(serde_json::Value::as_object)
        .unwrap();
    assert!(properties.contains_key("name"), "schema needs a name field");
    assert!(
        properties.contains_key("command"),
        "schema needs a command field"
    );
}

#[test]
fn command_output_renders_reply_or_notes_silence() {
    let with_output = CommandResponse {
        output: "There are 2 of a max of 20 players online".to_owned(),
    };
    let rendered = format_command_output("mc", "list", &with_output);
    assert!(rendered.contains("list"));
    assert!(rendered.contains("There are 2 of a max of 20 players online"));

    let empty = CommandResponse {
        output: "   \n".to_owned(),
    };
    assert!(
        format_command_output("mc", "say hi", &empty).contains("no output"),
        "a blank reply should be reported as no output"
    );
}

#[test]
fn name_param_schema_is_clean_object() {
    let schema = params_schema::<NameParams>();
    let object = schema.as_object().unwrap();
    assert_eq!(
        object.get("type").and_then(serde_json::Value::as_str),
        Some("object")
    );
    assert!(
        object
            .get("properties")
            .and_then(|properties| properties.get("name"))
            .is_some(),
        "the name parameter must be in the schema"
    );
    assert!(
        !object.contains_key("$schema") && !object.contains_key("title"),
        "provider-unfriendly metadata keys must be stripped"
    );
}

#[test]
fn server_summary_renders_game_state_and_address() {
    let summary = ServerSummary {
        name: "mc-abc12".to_owned(),
        game: Some("minecraft".to_owned()),
        state: "Ready".to_owned(),
        address: Some("mc-abc12.example.com:7000".to_owned()),
    };
    let rendered = format_summary(&summary);
    assert!(rendered.contains("mc-abc12"));
    assert!(rendered.contains("minecraft"));
    assert!(rendered.contains("Ready"));
    assert!(rendered.contains("mc-abc12.example.com:7000"));
}

#[test]
fn empty_server_list_reads_as_none() {
    assert_eq!(format_server_list(&[]), "no game servers exist right now");
}

#[test]
fn supervisor_outcomes_map_to_distinct_messages() {
    let paused = format_supervisor("mc", &SupervisorOutcome::Paused);
    let running = format_supervisor("mc", &SupervisorOutcome::AlreadyRunning);
    let missing = format_supervisor("mc", &SupervisorOutcome::NotFound);
    assert!(paused.contains("paused"));
    assert!(running.contains("already running"));
    assert_eq!(missing, "there's no server named mc");
}

#[test]
fn remove_outcomes_report_deletion_or_absence() {
    assert_eq!(
        format_remove("mc", &RemoveOutcome::Removed),
        "deleted mc and its world"
    );
    assert!(format_remove("mc", &RemoveOutcome::NotManaged).contains("managed by the platform"));
}

#[test]
fn fs_result_passes_payload_through_and_maps_problems() {
    assert_eq!(fs_result("mc", FsOutcome::Ok(42)), Ok(42));
    assert_eq!(
        fs_result::<()>("mc", FsOutcome::NotFound),
        Err("there's no server named mc".to_owned())
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::NotManaged)
            .unwrap_err()
            .contains("managed by the platform")
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::PodNotReady)
            .unwrap_err()
            .contains("isn't ready")
    );
    assert!(
        fs_result::<()>("mc", FsOutcome::Unreachable)
            .unwrap_err()
            .contains("couldn't reach")
    );
    assert_eq!(
        fs_result::<()>(
            "mc",
            FsOutcome::Rejected("path is outside the server's data directory".to_owned())
        ),
        Err("that didn't work: path is outside the server's data directory".to_owned()),
        "a supervisor rejection should be relayed verbatim after the lead-in"
    );
}

#[test]
fn browse_listing_describes_files_and_folders() {
    let entries = vec![
        DirEntry {
            name: "logs".to_owned(),
            kind: EntryKind::Dir,
            size: 0,
        },
        DirEntry {
            name: "server.properties".to_owned(),
            kind: EntryKind::File,
            size: 1024,
        },
    ];
    let rendered = format_entries("", &entries);
    assert!(rendered.contains("the data directory"));
    assert!(rendered.contains("logs/ (folder)"));
    assert!(rendered.contains("server.properties (1024 bytes)"));
}

#[test]
fn empty_directory_is_reported() {
    assert_eq!(format_entries("config", &[]), "config is empty");
}

#[test]
fn read_file_notes_truncation() {
    let whole = ReadResponse {
        path: "server.properties".to_owned(),
        content: "difficulty=hard".to_owned(),
        truncated: false,
    };
    assert!(!format_file(&whole).contains("truncated"));
    let cut = ReadResponse {
        path: "logs/latest.log".to_owned(),
        content: "...".to_owned(),
        truncated: true,
    };
    assert!(format_file(&cut).contains("truncated"));
}

#[test]
fn write_result_flags_whether_a_revert_is_possible() {
    let overwrite = WriteResponse {
        path: "server.properties".to_owned(),
        backed_up: true,
    };
    let rendered = format_write(&overwrite);
    assert!(rendered.contains("restore_file"));
    assert!(rendered.contains("restart"));
    let fresh = WriteResponse {
        path: "ops.json".to_owned(),
        backed_up: false,
    };
    assert!(format_write(&fresh).contains("nothing to restore"));
}

#[test]
fn logs_render_or_report_absence() {
    assert!(format_logs("mc", &[]).contains("hasn't produced any output"));
    let rendered = format_logs(
        "mc",
        &["[12:00] starting".to_owned(), "[12:01] ready".to_owned()],
    );
    assert!(rendered.contains("[12:00] starting"));
    assert!(rendered.contains("[12:01] ready"));
}
