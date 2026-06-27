#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

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
