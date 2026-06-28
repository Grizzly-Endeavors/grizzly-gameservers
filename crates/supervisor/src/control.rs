use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use grizzly_control_api::{
    ControlCommand, ControlError, ControlOk, ListResponse, LogsQuery, LogsResponse, PathQuery,
    ReadResponse, RestoreRequest, RestoreResponse, ResultKind, RouteError, StatusResponse,
    WriteRequest, WriteResponse,
};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error};

use crate::fs::{self, FsError};
use crate::logs::{DEFAULT_TAIL_LINES, LogBuffer};

/// A control request handed from the HTTP layer to the runner, with a one-shot
/// channel for the runner to answer on.
pub struct ControlRequest {
    pub command: ControlCommand,
    pub reply: oneshot::Sender<ControlReply>,
}

/// The runner's answer to a [`ControlRequest`].
pub enum ControlReply {
    /// A state-changing (or no-op) action completed.
    Acted(ResultKind),
    /// A status snapshot for `GET /status`.
    Status(StatusResponse),
    /// The action failed; the string is developer-facing.
    Failed(String),
}

#[derive(Clone)]
struct ControlState {
    tx: mpsc::Sender<ControlRequest>,
    /// Root the filesystem routes are confined to (the instance PVC mount).
    data_root: Arc<Path>,
    /// Tail of the game process's recent output, served by `GET /logs`.
    logs: Arc<LogBuffer>,
}

/// Serve the control API on `0.0.0.0:port` until the listener errors. The port is
/// pod-internal — never added to the `NodePort` Service — so only the bot
/// (allowed by a scoped Cilium egress rule) can reach it.
///
/// The lifecycle routes (stop/start/restart/status) are matched by the
/// [`ControlCommand`] fallback; the filesystem and logs routes carry bodies and
/// queries, so they're registered explicitly and handled in this layer rather
/// than handed to the runner.
///
/// # Errors
///
/// Returns an error if the port cannot be bound or the server loop fails.
pub async fn serve(
    port: u16,
    tx: mpsc::Sender<ControlRequest>,
    data_root: Arc<Path>,
    logs: Arc<LogBuffer>,
) -> Result<()> {
    let app = Router::new()
        .route("/fs/list", get(fs_list))
        .route("/fs/read", get(fs_read))
        .route("/fs/write", post(fs_write))
        .route("/fs/restore", post(fs_restore))
        .route("/logs", get(logs_tail))
        .fallback(any(handle))
        .with_state(ControlState {
            tx,
            data_root,
            logs,
        });
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind control api on {addr}"))?;
    axum::serve(listener, app)
        .await
        .context("control api server stopped")
}

async fn handle(State(state): State<ControlState>, request: Request) -> Response {
    let method = request.method().as_str();
    let path = request.uri().path();
    match ControlCommand::from_request(method, path) {
        Ok(command) => dispatch(&state, command).await,
        Err(RouteError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(ControlError::new("not found"))).into_response()
        }
        Err(RouteError::MethodNotAllowed) => (
            StatusCode::METHOD_NOT_ALLOWED,
            Json(ControlError::new("method not allowed")),
        )
            .into_response(),
    }
}

async fn dispatch(state: &ControlState, command: ControlCommand) -> Response {
    let (reply_tx, reply_rx) = oneshot::channel();
    let request = ControlRequest {
        command,
        reply: reply_tx,
    };
    if state.tx.send(request).await.is_err() {
        error!("runner is no longer accepting control requests");
        return internal_error("supervisor is shutting down");
    }
    match reply_rx.await {
        Ok(ControlReply::Acted(kind)) => {
            (StatusCode::OK, Json(ControlOk::new(kind))).into_response()
        }
        Ok(ControlReply::Status(status)) => (StatusCode::OK, Json(status)).into_response(),
        Ok(ControlReply::Failed(message)) => internal_error(message),
        Err(_) => internal_error("supervisor dropped the request"),
    }
}

fn internal_error(message: impl Into<String>) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ControlError::new(message)),
    )
        .into_response()
}

async fn fs_list(State(state): State<ControlState>, Query(query): Query<PathQuery>) -> Response {
    match fs::list_dir(&state.data_root, &query.path) {
        Ok(entries) => (
            StatusCode::OK,
            Json(ListResponse {
                path: query.path,
                entries,
            }),
        )
            .into_response(),
        Err(err) => fs_error("list", &query.path, &err),
    }
}

async fn fs_read(State(state): State<ControlState>, Query(query): Query<PathQuery>) -> Response {
    match fs::read_file(&state.data_root, &query.path) {
        Ok((content, truncated)) => (
            StatusCode::OK,
            Json(ReadResponse {
                path: query.path,
                content,
                truncated,
            }),
        )
            .into_response(),
        Err(err) => fs_error("read", &query.path, &err),
    }
}

async fn fs_write(State(state): State<ControlState>, Json(body): Json<WriteRequest>) -> Response {
    match fs::write_file(&state.data_root, &body.path, &body.content) {
        Ok(backed_up) => (
            StatusCode::OK,
            Json(WriteResponse {
                path: body.path,
                backed_up,
            }),
        )
            .into_response(),
        Err(err) => fs_error("write", &body.path, &err),
    }
}

async fn fs_restore(
    State(state): State<ControlState>,
    Json(body): Json<RestoreRequest>,
) -> Response {
    match fs::restore_file(&state.data_root, &body.path) {
        Ok(()) => (StatusCode::OK, Json(RestoreResponse { path: body.path })).into_response(),
        Err(err) => fs_error("restore", &body.path, &err),
    }
}

async fn logs_tail(State(state): State<ControlState>, Query(query): Query<LogsQuery>) -> Response {
    let count = query.lines.unwrap_or(DEFAULT_TAIL_LINES);
    let lines = state.logs.tail(count);
    (StatusCode::OK, Json(LogsResponse { lines })).into_response()
}

/// Map a filesystem error to an HTTP status + developer-facing body, logging IO
/// faults loudly and bad requests quietly. The bot translates the outcome into
/// friend-facing copy, so the message here stays diagnostic.
fn fs_error(op: &str, path: &str, err: &FsError) -> Response {
    let status = match err {
        FsError::OutsideRoot => StatusCode::FORBIDDEN,
        FsError::NotFound | FsError::NoBackup => StatusCode::NOT_FOUND,
        FsError::NotAFile | FsError::NotADirectory | FsError::NotText | FsError::TooLarge => {
            StatusCode::BAD_REQUEST
        }
        FsError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    if err.is_client_error() {
        debug!(op, path, error = %err, "rejected filesystem request");
    } else {
        error!(op, path, error = %err, "filesystem operation failed");
    }
    (status, Json(ControlError::new(err.to_string()))).into_response()
}
