use crate::agones::{DestroyOutcome, EditOutcome, FsOutcome, ReadyWait, SupervisorOutcome};

use super::*;

fn tool_names(access: AccessLevel) -> Vec<String> {
    available_tools(access)
        .into_iter()
        .map(|tool| tool.function.name)
        .collect()
}

/// The tools reserved for admins — never offered to a manager or read-only
/// caller. Kept next to the tier tests so a newly added admin tool is checked.
const ADMIN_ONLY: [&str; 5] = [
    DESTROY_SERVER,
    SEND_COMMAND,
    ARCHIVE_SERVER,
    RESTORE_SERVER,
    RECOVER_SERVER,
];

/// The lifecycle and file tools a manager gets on top of the read-only set.
const MANAGER_ADDED: [&str; 13] = [
    CREATE_SERVER,
    STOP_SERVER,
    START_SERVER,
    RESTART_SERVER,
    SHUTDOWN_SERVER,
    BROWSE_FILES,
    READ_FILE,
    READ_LOGS,
    WRITE_FILE,
    EDIT_FILE,
    RESTORE_FILE,
    WAIT_FOR_SERVER,
    BACKUP_SERVER,
];

#[test]
fn read_only_callers_get_only_the_read_only_tools() {
    let names = tool_names(AccessLevel::ReadOnly);
    assert_eq!(
        names,
        vec![LIST_SERVERS, SERVER_STATUS, LIST_BACKUPS, LIST_ARCHIVES]
    );
    for reserved in MANAGER_ADDED.iter().chain(ADMIN_ONLY.iter()) {
        assert!(
            !names.iter().any(|name| name == reserved),
            "read-only tier must not expose {reserved}"
        );
    }
}

#[test]
fn managers_get_lifecycle_and_files_but_not_the_destructive_tools() {
    let names = tool_names(AccessLevel::Manager);
    for expected in MANAGER_ADDED {
        assert!(
            names.iter().any(|name| name == expected),
            "managers should get {expected}"
        );
    }
    for reserved in ADMIN_ONLY {
        assert!(
            !names.iter().any(|name| name == reserved),
            "{reserved} must not be offered to managers"
        );
    }
    assert_eq!(
        names.len(),
        17,
        "four read tools plus the thirteen manager lifecycle/file tools"
    );
}

#[test]
fn admins_get_the_full_lifecycle_and_filesystem_set() {
    let names = tool_names(AccessLevel::Admin);
    for expected in MANAGER_ADDED.iter().chain(ADMIN_ONLY.iter()) {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
    assert_eq!(
        names.len(),
        22,
        "the four read tools, thirteen manager tools, and five admin-only tools"
    );
}

#[test]
fn scope_gate_covers_every_targeted_tool_and_spares_list_and_create() {
    // The gate must apply to every tool that names an existing server, and only
    // to those — list_servers/list_archives scope their own query, and
    // create_server/recover_server make a new server stamped with the channel.
    // Derived from the live tool set so a newly added tool can't slip past the
    // gate without this failing.
    for name in tool_names(AccessLevel::Admin) {
        let should_gate = name != LIST_SERVERS
            && name != CREATE_SERVER
            && name != LIST_ARCHIVES
            && name != RECOVER_SERVER;
        assert_eq!(
            targets_existing_server(&name),
            should_gate,
            "{name}: scope-gating classification is wrong"
        );
    }
}

#[test]
fn unknown_tool_is_not_scope_gated() {
    assert!(!targets_existing_server("frobnicate"));
}

#[test]
fn filesystem_tools_are_manager_and_up() {
    // File tools power day-to-day troubleshooting, so managers get them — but
    // read-only callers never do.
    let read_only = tool_names(AccessLevel::ReadOnly);
    let managers = tool_names(AccessLevel::Manager);
    for tool in [
        BROWSE_FILES,
        READ_FILE,
        READ_LOGS,
        WRITE_FILE,
        EDIT_FILE,
        RESTORE_FILE,
        WAIT_FOR_SERVER,
    ] {
        assert!(
            !read_only.iter().any(|name| name == tool),
            "{tool} must not be offered to read-only callers"
        );
        assert!(
            managers.iter().any(|name| name == tool),
            "{tool} should be offered to managers"
        );
    }
}

#[test]
fn send_command_is_admin_only() {
    for tier in [AccessLevel::ReadOnly, AccessLevel::Manager] {
        assert!(
            !tool_names(tier).iter().any(|name| name == SEND_COMMAND),
            "send_command must not be offered below the admin tier"
        );
    }
    assert!(
        tool_names(AccessLevel::Admin)
            .iter()
            .any(|name| name == SEND_COMMAND),
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
fn supervisor_outcomes_map_to_distinct_messages() {
    let paused = format_supervisor("mc", &SupervisorOutcome::Paused);
    let running = format_supervisor("mc", &SupervisorOutcome::AlreadyRunning);
    let missing = format_supervisor("mc", &SupervisorOutcome::NotFound);
    assert!(paused.contains("paused"));
    assert!(running.contains("already running"));
    assert_eq!(
        missing,
        "there's no server named mc — check list_servers for the current names"
    );
}

#[test]
fn supervisor_failed_relays_the_supervisors_reason() {
    let message = format_supervisor(
        "mc",
        &SupervisorOutcome::Failed("rcon is not enabled for this game".to_owned()),
    );
    assert!(
        message.contains("rcon is not enabled for this game"),
        "Gary should relay the supervisor's own reason, got: {message}"
    );
}

#[test]
fn remove_outcomes_report_deletion_or_absence() {
    assert_eq!(
        format_destroy("mc", &DestroyOutcome::Destroyed),
        "deleted mc and its world"
    );
    assert!(format_destroy("mc", &DestroyOutcome::NotManaged).contains("managed by the platform"));
}

#[test]
fn fs_result_passes_payload_through_and_maps_problems() {
    assert_eq!(fs_result("mc", FsOutcome::Ok(42)), Ok(42));
    assert_eq!(
        fs_result::<()>("mc", FsOutcome::NotFound),
        Err("there's no server named mc — check list_servers for the current names".to_owned())
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
fn edit_success_points_at_restart_and_undo() {
    let rendered = format_edit(
        "mc",
        "server.properties",
        EditOutcome::Edited(WriteResponse {
            path: "server.properties".to_owned(),
            backed_up: true,
        }),
    );
    assert!(rendered.contains("edited server.properties"));
    assert!(
        rendered.contains("restore_file"),
        "should mention how to undo"
    );
    assert!(rendered.contains("restart"), "should prompt a restart");
}

#[test]
fn edit_soft_failures_explain_the_recovery() {
    assert!(
        format_edit("mc", "server.properties", EditOutcome::NoMatch).contains("couldn't find"),
        "a missing anchor should tell Gary to re-read and match exactly"
    );
    let ambiguous = format_edit("mc", "server.properties", EditOutcome::Ambiguous(3));
    assert!(ambiguous.contains('3'), "ambiguity should report the count");
    assert!(
        format_edit("mc", "server.properties", EditOutcome::TooLargeToEdit).contains("write_file"),
        "an un-editable large file should point at the write_file fallback"
    );
    // A shared FS failure carried through Unserved renders like any other.
    assert_eq!(
        format_edit(
            "mc",
            "server.properties",
            EditOutcome::Unserved(FsOutcome::NotFound)
        ),
        "there's no server named mc — check list_servers for the current names"
    );
}

#[test]
fn ready_wait_outcomes_map_to_distinct_messages() {
    assert!(format_ready_wait("mc", &ReadyWait::Ready).contains("back up"));
    assert!(format_ready_wait("mc", &ReadyWait::Crashed).contains("crashed"));
    assert!(
        format_ready_wait("mc", &ReadyWait::Stopped).contains("stopped"),
        "a paused server won't come up on its own and should say so"
    );
    assert!(format_ready_wait("mc", &ReadyWait::TimedOut).contains("still isn't accepting"));
    assert_eq!(
        format_ready_wait("mc", &ReadyWait::NotFound),
        "there's no server named mc — check list_servers for the current names"
    );
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
