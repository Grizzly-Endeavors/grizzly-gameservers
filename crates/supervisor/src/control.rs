use std::net::{Ipv4Addr, SocketAddr};

use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use grizzly_control_api::{
    ControlCommand, ControlError, ControlOk, ResultKind, RouteError, StatusResponse,
};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tracing::error;

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
}

/// Serve the control API on `0.0.0.0:port` until the listener errors. The port is
/// pod-internal — never added to the `NodePort` Service — so only the bot
/// (allowed by a scoped Cilium egress rule) can reach it.
///
/// # Errors
///
/// Returns an error if the port cannot be bound or the server loop fails.
pub async fn serve(port: u16, tx: mpsc::Sender<ControlRequest>) -> Result<()> {
    let app = Router::new()
        .fallback(any(handle))
        .with_state(ControlState { tx });
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
