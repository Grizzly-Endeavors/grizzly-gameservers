use super::*;

#[test]
fn from_request_routes_each_command() {
    assert_eq!(
        ControlCommand::from_request("POST", "/stop"),
        Ok(ControlCommand::Stop),
        "POST /stop should route to Stop"
    );
    assert_eq!(
        ControlCommand::from_request("POST", "/start"),
        Ok(ControlCommand::Start),
        "POST /start should route to Start"
    );
    assert_eq!(
        ControlCommand::from_request("POST", "/restart"),
        Ok(ControlCommand::Restart),
        "POST /restart should route to Restart"
    );
    assert_eq!(
        ControlCommand::from_request("GET", "/status"),
        Ok(ControlCommand::Status),
        "GET /status should route to Status"
    );
}

#[test]
fn from_request_rejects_unknown_path_with_not_found() {
    assert_eq!(
        ControlCommand::from_request("POST", "/nope"),
        Err(RouteError::NotFound),
        "an unrouted path is NotFound"
    );
}

#[test]
fn from_request_rejects_wrong_method_with_method_not_allowed() {
    assert_eq!(
        ControlCommand::from_request("GET", "/stop"),
        Err(RouteError::MethodNotAllowed),
        "/stop is POST-only"
    );
    assert_eq!(
        ControlCommand::from_request("POST", "/status"),
        Err(RouteError::MethodNotAllowed),
        "/status is GET-only"
    );
}

#[test]
fn path_and_method_round_trip_through_from_request() {
    for command in [
        ControlCommand::Stop,
        ControlCommand::Start,
        ControlCommand::Restart,
        ControlCommand::Status,
    ] {
        assert_eq!(
            ControlCommand::from_request(command.method(), command.path()),
            Ok(command),
            "{command:?} should round-trip through its own method+path"
        );
    }
}

#[test]
fn control_ok_serializes_to_result_object() {
    let json = serde_json::to_string(&ControlOk::new(ResultKind::AlreadyStopped)).unwrap();
    assert_eq!(
        json, r#"{"result":"already_stopped"}"#,
        "ControlOk should be a snake_case result object"
    );
}

#[test]
fn control_error_serializes_to_error_object() {
    let json = serde_json::to_string(&ControlError::new("pod not ready")).unwrap();
    assert_eq!(
        json, r#"{"error":"pod not ready"}"#,
        "ControlError should be an error object"
    );
}

#[test]
fn status_response_round_trips() {
    let status = StatusResponse {
        process: ProcessPhase::Stopped,
        ready: true,
        pid: None,
        uptime_seconds: 0,
        restarts: 2,
    };
    let json = serde_json::to_string(&status).unwrap();
    let parsed: StatusResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed, status,
        "StatusResponse should survive a serde round-trip"
    );
}

#[test]
fn process_phase_serializes_snake_case() {
    let json = serde_json::to_string(&ProcessPhase::Crashed).unwrap();
    assert_eq!(json, r#""crashed""#, "phase should be a snake_case string");
}

#[test]
fn entry_kind_serializes_snake_case() {
    assert_eq!(
        serde_json::to_string(&EntryKind::File).unwrap(),
        r#""file""#
    );
    assert_eq!(serde_json::to_string(&EntryKind::Dir).unwrap(), r#""dir""#);
    assert_eq!(
        serde_json::to_string(&EntryKind::Other).unwrap(),
        r#""other""#
    );
}

#[test]
fn list_response_round_trips() {
    let response = ListResponse {
        path: "logs".to_owned(),
        entries: vec![
            DirEntry {
                name: "latest.log".to_owned(),
                kind: EntryKind::File,
                size: 4096,
            },
            DirEntry {
                name: "archive".to_owned(),
                kind: EntryKind::Dir,
                size: 0,
            },
        ],
    };
    let json = serde_json::to_string(&response).unwrap();
    let parsed: ListResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, response, "ListResponse should survive a round-trip");
}

#[test]
fn read_response_round_trips() {
    let response = ReadResponse {
        path: "server.properties".to_owned(),
        content: "difficulty=hard\n".to_owned(),
        truncated: false,
    };
    let json = serde_json::to_string(&response).unwrap();
    let parsed: ReadResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, response, "ReadResponse should survive a round-trip");
}

#[test]
fn write_request_and_response_round_trip() {
    let request = WriteRequest {
        path: "server.properties".to_owned(),
        content: "difficulty=peaceful\n".to_owned(),
    };
    let request_json = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<WriteRequest>(&request_json).unwrap(),
        request
    );

    let response = WriteResponse {
        path: "server.properties".to_owned(),
        backed_up: true,
    };
    let response_json = serde_json::to_string(&response).unwrap();
    assert_eq!(
        serde_json::from_str::<WriteResponse>(&response_json).unwrap(),
        response
    );
}

#[test]
fn restore_request_and_response_round_trip() {
    let request = RestoreRequest {
        path: "server.properties".to_owned(),
    };
    let request_json = serde_json::to_string(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<RestoreRequest>(&request_json).unwrap(),
        request
    );

    let response = RestoreResponse {
        path: "server.properties".to_owned(),
    };
    let response_json = serde_json::to_string(&response).unwrap();
    assert_eq!(
        serde_json::from_str::<RestoreResponse>(&response_json).unwrap(),
        response
    );
}

#[test]
fn logs_query_defaults_lines_to_none() {
    let parsed: LogsQuery = serde_json::from_str("{}").unwrap();
    assert_eq!(
        parsed,
        LogsQuery { lines: None },
        "an absent lines field should default to None"
    );
}

#[test]
fn logs_response_round_trips() {
    let response = LogsResponse {
        lines: vec![
            "[12:00:00] starting".to_owned(),
            "[12:00:05] ready".to_owned(),
        ],
    };
    let json = serde_json::to_string(&response).unwrap();
    let parsed: LogsResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, response, "LogsResponse should survive a round-trip");
}
