#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use crate::agones::{RemoveOutcome, ServerSummary, SupervisorOutcome};

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
fn admins_get_the_full_lifecycle_set() {
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
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
    assert_eq!(names.len(), 8, "exactly the eight lifecycle tools");
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
