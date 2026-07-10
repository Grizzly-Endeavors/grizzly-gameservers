//! The in-game agent endpoint: a small HTTP server the game-pod supervisors POST
//! player `@Gary` chat triggers to. It authenticates the caller with a shared
//! bearer token, resolves the server to its guild scope, and hands the question
//! to the read-only [`agent`] orchestrator — which answers and broadcasts the
//! reply back into the game over RCON. This is the bot half of the reverse loop;
//! the inbound parsing lives in the supervisor's chat watcher.
//!
//! It runs alongside the Discord gateway in the same process, sharing Gary's core
//! and session store via [`IngameDeps`] (cloneable handles carved from the
//! command `Data`, the same pattern the backup cycle uses).

mod agent;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use grizzly_control_api::{INGAME_TRIGGER_PATH, IngameTriggerRequest};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{error, info, warn};

use crate::agent::{OllamaConfig, SessionStore};
use crate::agones::GameCatalog;

/// Path of the always-on liveness/readiness route. Only the kubelet probe
/// consumes it, so — unlike [`INGAME_TRIGGER_PATH`] — it isn't part of the
/// control-api wire contract and stays local to this module.
const HEALTH_PATH: &str = "/health";

/// Cloneable handles the in-game endpoint shares with the rest of the bot. Every
/// field is a cheap clone (an `Arc` bump or a short owned string), so the endpoint
/// and the Discord shell reach the same cluster client, catalog, and session
/// store without wrapping the whole command `Data` in an `Arc`.
#[derive(Clone)]
pub(crate) struct IngameDeps {
    pub(crate) client: kube::Client,
    pub(crate) http: reqwest::Client,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Gary's model connection. `None` disables the endpoint entirely (nothing to
    /// answer with), so [`spawn`] doesn't start it.
    pub(crate) ollama: Option<OllamaConfig>,
    pub(crate) sessions: Arc<SessionStore>,
}

/// The endpoint's shared state: the dependencies plus the expected bearer token
/// (`None` runs it open, protected by `NetworkPolicy` alone).
#[derive(Clone)]
struct IngameServer {
    deps: IngameDeps,
    token: Option<Arc<str>>,
    /// Tracks the detached per-trigger answer tasks so the shutdown drain awaits
    /// an in-flight in-game answer instead of dropping it on process exit.
    tasks: TaskTracker,
}

/// Start the in-game agent endpoint in a background task. The server always comes
/// up so the k8s probes have a `GET /health` target; the Gary-backed
/// [`INGAME_TRIGGER_PATH`] route is mounted only when Gary is configured (nothing
/// to answer with otherwise). Warns when the trigger route is live but has no
/// token — it is then only network-isolated.
pub(crate) fn spawn(
    deps: IngameDeps,
    port: u16,
    token: Option<String>,
    tasks: &TaskTracker,
    shutdown: CancellationToken,
) {
    if deps.ollama.is_none() {
        info!("in-game trigger route disabled (Gary not configured); serving health only");
    } else if token.is_none() {
        warn!(
            "in-game agent endpoint has no token (GAMESERVERS_INGAME_TOKEN unset); it is only \
             protected by network policy"
        );
    }
    let tracker = tasks.clone();
    tasks.spawn(async move {
        if let Err(err) = serve(deps, port, token, shutdown, tracker).await {
            error!(error = ?err, "in-game agent endpoint terminated");
        }
    });
}

/// Bind the endpoint and serve until `shutdown` is cancelled, draining in-flight
/// requests (axum's graceful shutdown) and, via the shared tracker, the detached
/// answer tasks each trigger spawns.
async fn serve(
    deps: IngameDeps,
    port: u16,
    token: Option<String>,
    shutdown: CancellationToken,
    tasks: TaskTracker,
) -> Result<()> {
    let gary_enabled = deps.ollama.is_some();
    // `GET /health` is always served: unauthenticated, stateless, and independent
    // of Gary, so the k8s probes have a target whether or not the token-guarded
    // trigger route is mounted. The auth layer is scoped to the trigger sub-router
    // alone — a probe carries no bearer token and must not be turned away.
    let mut app: Router = Router::new().route(HEALTH_PATH, get(health));
    if gary_enabled {
        let state = IngameServer {
            deps,
            token: token.map(Arc::from),
            tasks,
        };
        let trigger = Router::new()
            .route(INGAME_TRIGGER_PATH, post(ingame_trigger))
            .layer(from_fn_with_state(state.clone(), require_token))
            .with_state(state);
        app = app.merge(trigger);
    }

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind in-game agent endpoint on {addr}"))?;
    info!(port, gary_enabled, "in-game agent endpoint listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("in-game agent endpoint server failed")?;
    Ok(())
}

/// Auth middleware: reject anything without the shared bearer token *before* the
/// body is parsed, so an unauthenticated caller can't reach the handler. Runs open
/// when no token is configured.
async fn require_token(
    State(server): State<IngameServer>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if authorized(server.token.as_deref(), request.headers()) {
        next.run(request).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Whether `headers` carry the expected bearer token. `None` expected means the
/// endpoint runs open (network-isolated only). The comparison is constant-time so
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

/// Accept a trigger, answer it asynchronously, and return `202` immediately — the
/// reply is delivered back over RCON, so the supervisor's POST never waits on
/// Gary's turn.
async fn ingame_trigger(
    State(server): State<IngameServer>,
    axum::Json(request): axum::Json<IngameTriggerRequest>,
) -> StatusCode {
    let deps = server.deps;
    server.tasks.spawn(async move {
        agent::handle_ingame_question(&deps, &request.server, &request.player, &request.message)
            .await;
    });
    StatusCode::ACCEPTED
}

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
