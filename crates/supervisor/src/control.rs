use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use grizzly_control_api::{
    ARCHIVE_PATH, AnnounceRequest, ArchiveQuery, CommandRequest, CommandResponse, ControlCommand,
    ControlError, ControlOk, ExtractQuery, ListResponse, LogsQuery, LogsResponse, PathQuery,
    ReadResponse, RestoreRequest, RestoreResponse, ResultKind, RouteError, StatusResponse,
    WriteRequest, WriteResponse,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::StreamExt as _;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, warn};

use crate::archive;
use crate::fs::{self, FsError};
use crate::logs::{DEFAULT_TAIL_LINES, LogBuffer};
use crate::rcon::RconRuntime;

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
    /// RCON client for `POST /command`, or `None` when the game doesn't enable it.
    rcon: Option<Arc<RconRuntime>>,
}

/// Serve the control API on `0.0.0.0:port` until the listener errors. The port is
/// pod-internal — never added to the `NodePort` Service — so only the bot
/// (allowed by a scoped Cilium egress rule) can reach it.
///
/// The lifecycle routes (stop/start/restart/status) are matched by the
/// [`ControlCommand`] fallback; the filesystem, logs, and command routes carry
/// bodies and queries, so they're registered explicitly and handled in this layer
/// rather than handed to the runner. `POST /command` runs entirely here — it
/// speaks RCON to the game over loopback, independent of the child-process
/// lifecycle the runner owns.
///
/// # Errors
///
/// Returns an error if the port cannot be bound or the server loop fails.
pub async fn serve(
    port: u16,
    tx: mpsc::Sender<ControlRequest>,
    data_root: Arc<Path>,
    logs: Arc<LogBuffer>,
    rcon: Option<Arc<RconRuntime>>,
) -> Result<()> {
    let app = Router::new()
        .route("/fs/list", get(fs_list))
        .route("/fs/read", get(fs_read))
        .route("/fs/write", post(fs_write))
        .route("/fs/restore", post(fs_restore))
        .route(ARCHIVE_PATH, get(archive_out).post(archive_in))
        .route("/logs", get(logs_tail))
        .route("/command", post(run_command))
        .route("/announce", post(announce))
        .fallback(any(handle))
        .with_state(ControlState {
            tx,
            data_root,
            logs,
            rcon,
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

/// Stream a zstd-compressed tar of the whole data root to the caller. When
/// `quiesce` is set, world saves are flushed and paused before the snapshot and
/// re-enabled once `tar` finishes (in the reaper task), so a *live* backup is
/// internally consistent. The child is reaped — and saves re-enabled — off the
/// response future so an early client disconnect still resolves cleanly.
async fn archive_out(
    State(state): State<ControlState>,
    Query(query): Query<ArchiveQuery>,
) -> Response {
    let rcon = state.rcon.clone();
    if query.quiesce
        && let Some(runtime) = rcon.as_ref()
        && let Err(err) = runtime.quiesce_for_snapshot().await
    {
        // Best-effort: an un-quiesced snapshot is still usable, and the reaper
        // re-enables saving regardless of this outcome.
        warn!(error = ?err, "failed to quiesce before snapshot; proceeding");
    }

    let mut child = match archive::spawn_create(&state.data_root) {
        Ok(child) => child,
        Err(err) => {
            error!(error = ?err, "failed to start data archive");
            resume_after_snapshot(query.quiesce, rcon.as_deref()).await;
            return internal_error("failed to spawn tar for the archive stream");
        }
    };
    let Some(stdout) = child.stdout.take() else {
        error!("tar create produced no stdout pipe");
        resume_after_snapshot(query.quiesce, rcon.as_deref()).await;
        return internal_error("archive tar produced no stdout pipe");
    };
    let stderr = child.stderr.take();
    let quiesce = query.quiesce;

    tokio::spawn(async move {
        let stderr_tail = read_stderr_tail(stderr).await;
        match child.wait().await {
            Ok(status) if !status.success() => {
                error!(?status, stderr = %stderr_tail, "tar create exited non-zero");
            }
            Err(err) => error!(error = ?err, "failed to reap tar create"),
            Ok(_) => {}
        }
        resume_after_snapshot(quiesce, rcon.as_deref()).await;
    });

    (
        [(header::CONTENT_TYPE, "application/x-tar+zst")],
        Body::from_stream(ReaderStream::new(stdout)),
    )
        .into_response()
}

/// Extract an uploaded zstd tar stream into the data root, optionally purging it
/// first (overwrite-restore). The body streams straight into `tar`'s stdin so a
/// multi-gigabyte world never buffers in memory.
async fn archive_in(
    State(state): State<ControlState>,
    Query(query): Query<ExtractQuery>,
    request: Request,
) -> Response {
    if query.purge
        && let Err(err) = archive::purge(&state.data_root)
    {
        error!(error = ?err, "failed to purge data root before restore");
        return internal_error("failed to purge data root before restore");
    }

    let mut child = match archive::spawn_extract(&state.data_root) {
        Ok(child) => child,
        Err(err) => {
            error!(error = ?err, "failed to start data restore");
            return internal_error("failed to spawn tar for the restore extract");
        }
    };
    let Some(mut stdin) = child.stdin.take() else {
        error!("tar extract produced no stdin pipe");
        return internal_error("restore tar produced no stdin pipe");
    };
    let stderr = child.stderr.take();

    let mut body = request.into_body().into_data_stream();
    let mut receive_error: Option<String> = None;
    while let Some(chunk) = body.next().await {
        match chunk {
            Ok(bytes) => {
                if let Err(err) = stdin.write_all(&bytes).await {
                    receive_error = Some(err.to_string());
                    break;
                }
            }
            Err(err) => {
                receive_error = Some(err.to_string());
                break;
            }
        }
    }
    // Close stdin so tar sees EOF and finishes (or aborts, if we broke early).
    drop(stdin);
    let stderr_tail = read_stderr_tail(stderr).await;
    let status = child.wait().await;

    if let Some(err) = receive_error {
        error!(error = %err, "failed streaming archive into tar");
        return internal_error("failed to stream the archive body into tar");
    }
    match status {
        Ok(status) if status.success() => StatusCode::OK.into_response(),
        Ok(status) => {
            error!(?status, stderr = %stderr_tail, "tar extract exited non-zero");
            internal_error("tar extract exited non-zero")
        }
        Err(err) => {
            error!(error = ?err, "failed to reap tar extract");
            internal_error("failed to reap tar extract")
        }
    }
}

/// Re-enable world saves after a snapshot, undoing [`RconRuntime::quiesce_for_snapshot`].
/// A no-op when the snapshot wasn't quiesced or the game has no RCON.
async fn resume_after_snapshot(quiesced: bool, rcon: Option<&RconRuntime>) {
    if quiesced
        && let Some(runtime) = rcon
        && let Err(err) = runtime.resume_saves().await
    {
        warn!(error = ?err, "failed to re-enable world saves after snapshot");
    }
}

/// Drain a child's stderr to a string for diagnostics, so a failed `tar` reports
/// why. Best-effort: a read failure logs at debug and yields what was read.
async fn read_stderr_tail<R>(stderr: Option<R>) -> String
where
    R: AsyncReadExt + Unpin,
{
    let Some(mut stderr) = stderr else {
        return String::new();
    };
    let mut buf = String::new();
    if let Err(err) = stderr.read_to_string(&mut buf).await {
        debug!(error = ?err, "failed reading tar stderr");
    }
    buf.trim().to_owned()
}

async fn logs_tail(State(state): State<ControlState>, Query(query): Query<LogsQuery>) -> Response {
    let count = query.lines.unwrap_or(DEFAULT_TAIL_LINES);
    let lines = state.logs.tail(count);
    (StatusCode::OK, Json(LogsResponse { lines })).into_response()
}

/// Run one in-game console command over RCON. Returns 409 when the game doesn't
/// enable RCON, and 500 (with a diagnostic log) when the command can't be
/// delivered — a paused or unreachable game console lands here. The bot relays
/// the message; the developer-facing detail stays in the log.
async fn run_command(
    State(state): State<ControlState>,
    Json(body): Json<CommandRequest>,
) -> Response {
    let Some(rcon) = state.rcon.as_ref() else {
        return (
            StatusCode::CONFLICT,
            Json(ControlError::new("rcon is not enabled for this game")),
        )
            .into_response();
    };
    match rcon.run_command(&body.command).await {
        Ok(output) => (StatusCode::OK, Json(CommandResponse { output })).into_response(),
        Err(err) => {
            warn!(error = ?err, command = %body.command, "rcon command failed");
            internal_error(
                "the server's console isn't responding yet — it may still be starting up, so try again in a moment",
            )
        }
    }
}

/// Broadcast a message to everyone on the server over RCON. Returns 409 when the
/// game doesn't enable RCON (the bot treats this as best-effort and moves on), and
/// 500 when the console can't be reached. The bot never blocks a real action on
/// this route's outcome.
async fn announce(
    State(state): State<ControlState>,
    Json(body): Json<AnnounceRequest>,
) -> Response {
    let Some(rcon) = state.rcon.as_ref() else {
        return (
            StatusCode::CONFLICT,
            Json(ControlError::new(
                "announcements aren't supported for this game",
            )),
        )
            .into_response();
    };
    match rcon.broadcast(&body.message).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(err) => {
            warn!(error = ?err, message = %body.message, "rcon announce failed");
            internal_error("failed to broadcast over rcon")
        }
    }
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

#[cfg(test)]
#[path = "tests/control.rs"]
mod tests;
