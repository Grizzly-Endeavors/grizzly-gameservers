//! The bot's inbound HTTP surface: one axum server, several routers.
//!
//! This module is the transport — it binds the socket, serves the always-on
//! `GET /health` the kubelet probes, owns the shared bearer-token gate, and
//! merges the feature routers onto one listener. A router itself lives with the
//! feature it serves: the in-game trigger path stays in [`crate::ingame`], which
//! also owns its serenity-free answer orchestrator.
//!
//! How a router degrades without its token is decided at the mount site rather
//! than inside the gate, so each feature states its own posture.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};

use crate::ingame::IngameDeps;

/// Path of the always-on liveness/readiness route. Only the kubelet probe
/// consumes it, so — unlike the feature routes' paths — it isn't part of the
/// control-api wire contract and stays local to this module.
const HEALTH_PATH: &str = "/health";

/// Everything the HTTP server needs to mount its routers.
pub(crate) struct HttpDeps {
    pub(crate) ingame: IngameDeps,
    pub(crate) port: u16,
    /// Shared bearer token for the in-game trigger route. `None` runs that route
    /// open, protected by `NetworkPolicy` alone.
    pub(crate) ingame_token: Option<String>,
}

/// The expected bearer token for one router. `None` means that router runs open.
#[derive(Clone)]
pub(crate) struct BearerGate {
    expected: Option<Arc<str>>,
}

/// Start the HTTP server in a background task. `GET /health` always comes up so
/// the k8s probes have a target; each feature route mounts only when its own
/// preconditions are met.
pub(crate) fn spawn(deps: HttpDeps, tasks: &TaskTracker, shutdown: CancellationToken) {
    let tracker = tasks.clone();
    tasks.spawn(async move {
        if let Err(err) = serve(deps, shutdown, tracker).await {
            error!(error = ?err, "agent http endpoint terminated");
        }
    });
}

/// Bind the endpoint and serve until `shutdown` is cancelled, draining in-flight
/// requests (axum's graceful shutdown) and, via the shared tracker, the detached
/// tasks the feature routes spawn.
///
/// # Errors
///
/// Returns an error if the listener can't bind the port or the server loop fails.
async fn serve(deps: HttpDeps, shutdown: CancellationToken, tasks: TaskTracker) -> Result<()> {
    let HttpDeps {
        ingame,
        port,
        ingame_token,
    } = deps;

    // `GET /health` is always served: unauthenticated, stateless, and independent
    // of Gary, so the k8s probes have a target whether or not the token-guarded
    // feature routes are mounted. Each gate is scoped to its own sub-router — a
    // probe carries no bearer token and must not be turned away.
    let mut app: Router = Router::new().route(HEALTH_PATH, get(health));

    let ingame_route = ingame.ollama.is_some();
    if ingame_route {
        if ingame_token.is_none() {
            warn!(
                "in-game agent endpoint has no token (GAMESERVERS_INGAME_TOKEN unset); it is only \
                 protected by network policy"
            );
        }
        app = app.merge(crate::ingame::router(ingame, ingame_token, tasks));
    } else {
        info!("in-game trigger route disabled (Gary not configured)");
    }

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind agent http endpoint on {addr}"))?;
    info!(port, ingame_route, "agent http endpoint listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("agent http endpoint server failed")?;
    Ok(())
}

impl BearerGate {
    /// A gate expecting `token`. `None` runs the guarded router open.
    pub(crate) fn new(token: Option<String>) -> Self {
        Self {
            expected: token.map(Arc::from),
        }
    }
}

/// Auth middleware: reject anything without the expected bearer token *before*
/// the body is parsed, so an unauthenticated caller can't reach the handler.
/// Runs open when the gate carries no token.
pub(crate) async fn require_token(
    State(gate): State<BearerGate>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if authorized(gate.expected.as_deref(), request.headers()) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Layer [`require_token`] onto `router`, gated on `token`.
pub(crate) fn gated(router: Router, token: Option<String>) -> Router {
    let gate = BearerGate::new(token);
    router.layer(from_fn_with_state(gate, require_token))
}

/// Whether `headers` carry the expected bearer token. `None` expected means the
/// route runs open (network-isolated only). The comparison is constant-time so
/// a valid-length guess can't be narrowed by timing.
fn authorized(expected: Option<&str>, headers: &HeaderMap) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let Some(provided) = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return false;
    };
    constant_time_eq(provided.as_bytes(), expected.as_bytes())
}

/// Length-checked constant-time byte comparison. The length short-circuit leaks
/// only the token's length, which is not secret; the byte loop does not leak
/// where a same-length token first differs.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Liveness/readiness target for the k8s probes: `200` whenever the process is
/// serving. It does no work and touches no state on purpose — an unconfigured Gary
/// or an idle Discord gateway must not read as dead, and a genuinely wedged process
/// (what the liveness probe exists to catch) stops answering regardless.
async fn health() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
#[path = "tests/http.rs"]
mod tests;
