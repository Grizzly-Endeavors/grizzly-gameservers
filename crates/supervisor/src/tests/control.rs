use super::*;

use std::path::PathBuf;

#[tokio::test]
async fn fs_error_maps_each_variant_to_the_right_status() {
    let cases = [
        (FsError::OutsideRoot, StatusCode::FORBIDDEN),
        (FsError::NotFound, StatusCode::NOT_FOUND),
        (FsError::NoBackup, StatusCode::NOT_FOUND),
        (FsError::NotAFile, StatusCode::BAD_REQUEST),
        (FsError::NotADirectory, StatusCode::BAD_REQUEST),
        (FsError::NotText, StatusCode::BAD_REQUEST),
        (FsError::TooLarge, StatusCode::BAD_REQUEST),
        (
            FsError::Io("disk on fire".to_owned()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];

    for (err, expected) in cases {
        let response = fs_error("read", "/world/level.dat", &err);
        assert_eq!(
            response.status(),
            expected,
            "{err:?} should map to {expected}"
        );
    }
}

/// A [`ControlState`] for a game whose per-game template doesn't enable RCON.
/// `/command` and `/announce` must reject before ever touching `tx` or
/// `data_root`, so placeholder values are fine here.
fn state_without_rcon() -> ControlState {
    let (tx, _rx) = mpsc::channel::<ControlRequest>(1);
    ControlState {
        tx,
        data_root: Arc::from(PathBuf::from("/data")),
        logs: Arc::new(LogBuffer::new()),
        rcon: None,
    }
}

/// Decode a control route's JSON error body, so a test can assert on the
/// message the route actually sends rather than trusting the doc comment.
async fn body_error(response: Response) -> ControlError {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read response body");
    serde_json::from_slice(&bytes).expect("decode response body as a ControlError")
}

// `grizzly-control-api`'s `ControlError` doc (crates/control-api/src/lib.rs)
// says `/command`'s body is relayed to the friend near-verbatim, so its exact
// wording is part of the wire contract, not just diagnostics. Pin both routes'
// RCON-disabled reply here so a refactor that changes the status or the
// message fails a test instead of silently changing what the bot forwards.

#[tokio::test]
async fn run_command_rejects_with_conflict_when_rcon_disabled() {
    let response = run_command(
        State(state_without_rcon()),
        Json(CommandRequest {
            command: "list".to_owned(),
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = body_error(response).await;
    assert_eq!(
        error.error,
        "console commands aren't supported for this game"
    );
}

#[tokio::test]
async fn announce_rejects_with_conflict_when_rcon_disabled() {
    let response = announce(
        State(state_without_rcon()),
        Json(AnnounceRequest {
            message: "hello".to_owned(),
        }),
    )
    .await;

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error = body_error(response).await;
    assert_eq!(error.error, "announcements aren't supported for this game");
}
