use super::*;

fn offered(access: AccessLevel) -> Vec<String> {
    supervisor_only_tools(access)
        .into_iter()
        .map(|tool| tool.function.name)
        .collect()
}

/// Every tool that needs Kubernetes, Postgres, S3, Valkey or a Discord channel.
const NEEDS_MORE_THAN_A_SUPERVISOR: &[&str] = &[
    "create_server",
    "destroy_server",
    "start_server",
    "shutdown_server",
    "server_resources",
    "set_resources",
    "backup_server",
    "list_backups",
    "list_archives",
    "archive_server",
    "restore_server",
    "recover_server",
    "delete_archive",
    "run_when",
    "remember",
    "forget",
];

#[test]
fn the_local_tool_set_leaves_out_everything_that_needs_the_cluster() {
    let offered = offered(AccessLevel::Admin);
    for name in NEEDS_MORE_THAN_A_SUPERVISOR {
        assert!(known_tool(name), "{name} is not a tool; the list is stale");
        assert!(
            !offered.iter().any(|tool| tool == name),
            "{name} is offered"
        );
    }
}

#[test]
fn the_local_tool_set_keeps_the_supervisor_tools_at_admin() {
    let offered = offered(AccessLevel::Admin);
    for name in [
        "list_servers",
        "about_server",
        "read_file",
        "browse_files",
        "write_file",
        "edit_file",
        "restore_file",
        "read_logs",
        "run_shell",
        "send_command",
        "stop_server",
        "restart_server",
        "export_profile",
        "import_profile",
        "update_mods",
        "game_guide",
        "search_mods",
        "mod_info",
        "escalate",
    ] {
        assert!(offered.iter().any(|tool| tool == name), "{name} is missing");
    }
}

#[test]
fn the_local_tool_set_still_respects_the_tier() {
    let offered = offered(AccessLevel::ReadOnly);
    assert!(offered.iter().any(|tool| tool == "list_servers"));
    assert!(!offered.iter().any(|tool| tool == "write_file"));
    assert!(!offered.iter().any(|tool| tool == "send_command"));
}

#[test]
fn a_request_with_a_prompt_is_a_turn() {
    let job = job_of(
        Some("what's running?".to_owned()),
        None,
        None,
        serde_json::Value::Null,
    )
    .unwrap();
    assert!(matches!(job, Job::Turn { .. }));
}

#[test]
fn a_request_naming_a_supervisor_tool_is_a_tool_call() {
    let job = job_of(
        None,
        Some("read_logs".to_owned()),
        None,
        serde_json::json!({"name": "ev-x"}),
    )
    .unwrap();
    assert!(matches!(job, Job::Tool { .. }));
}

#[test]
fn a_cluster_tool_is_refused_up_front() {
    let err = job_of(
        None,
        Some("create_server".to_owned()),
        None,
        serde_json::Value::Null,
    )
    .err()
    .unwrap();
    assert!(err.to_string().contains("create_server"), "{err}");
}

#[test]
fn an_unknown_tool_is_refused_up_front() {
    assert!(
        job_of(
            None,
            Some("reed_file".to_owned()),
            None,
            serde_json::Value::Null
        )
        .is_err()
    );
}

#[test]
fn a_request_must_ask_for_exactly_one_thing() {
    assert!(
        job_of(
            Some("hi".to_owned()),
            Some("read_logs".to_owned()),
            None,
            serde_json::Value::Null
        )
        .is_err()
    );
    assert!(job_of(None, None, None, serde_json::Value::Null).is_err());
    assert!(job_of(Some("  ".to_owned()), None, None, serde_json::Value::Null).is_err());
}

#[test]
fn the_contract_request_shapes_parse() {
    let turn: LocalRequest = serde_json::from_str(
        r#"{"servers":[{"name":"ev-x","game":"factorio","control_url":"http://127.0.0.1:19359"}],
            "prompt":"hello","access":"admin",
            "model_profile":{"provider":"fireworks","model":"accounts/fireworks/models/glm-5p3-flash","max_tokens":null},
            "memory":false}"#,
    )
    .unwrap();
    assert_eq!(turn.servers.len(), 1);
    assert!(turn.model_profile.is_some());

    let tool: LocalRequest = serde_json::from_str(
        r#"{"servers":[],"tool":"write_file","arguments":{"name":"ev-x"},"access":"admin"}"#,
    )
    .unwrap();
    assert_eq!(tool.tool.as_deref(), Some("write_file"));
    assert!(!tool.memory);
}
