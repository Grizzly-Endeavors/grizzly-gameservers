pub(crate) mod operator;
pub(crate) mod signup;

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

const HEALTH_PATH: &str = "/health";

pub(crate) struct EndpointTokens {
    pub(crate) ingame: Option<String>,
    pub(crate) operator: Option<String>,
    pub(crate) signup: Option<String>,
}

pub(crate) struct HttpDeps {
    pub(crate) ingame: IngameDeps,
    pub(crate) operator: operator::OperatorDeps,
    pub(crate) signup: signup::SignupDeps,
    pub(crate) port: u16,
    pub(crate) ingame_token: Option<String>,
    pub(crate) operator_token: Option<String>,
    pub(crate) signup_token: Option<String>,
    pub(crate) cycle_health: Arc<crate::cycles::CycleHealth>,
}

impl HttpDeps {
    pub(crate) fn from_data(
        data: &crate::discord::Data,
        port: u16,
        tokens: EndpointTokens,
        cycle_health: Arc<crate::cycles::CycleHealth>,
    ) -> Self {
        let EndpointTokens {
            ingame,
            operator,
            signup,
        } = tokens;
        Self {
            ingame: IngameDeps::from_data(data),
            operator: operator::OperatorDeps::from_data(data),
            signup: signup::SignupDeps::from_data(data),
            port,
            ingame_token: ingame,
            operator_token: operator,
            signup_token: signup,
            cycle_health,
        }
    }
}

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

async fn serve(deps: HttpDeps, shutdown: CancellationToken, tasks: TaskTracker) -> Result<()> {
    let HttpDeps {
        ingame,
        operator,
        signup,
        port,
        ingame_token,
        operator_token,
        signup_token,
        cycle_health,
    } = deps;

    let mut app: Router = Router::new().route(
        HEALTH_PATH,
        get({
            let health_state = Arc::clone(&cycle_health);
            move || {
                let state = Arc::clone(&health_state);
                async move { health(&state) }
            }
        }),
    );

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

    let operator_routes = operator_token.is_some();
    if let Some(token) = operator_token {
        app = app.merge(operator::router(operator, token));
    } else {
        info!("operator routes disabled (GAMESERVERS_OPERATOR_TOKEN unset)");
    }

    let signup_route = match signup_token {
        Some(token) if signup.is_enabled() => {
            app = app.merge(signup::router(signup, token));
            true
        }
        Some(_) => {
            warn!(
                "signup intake route disabled: a token is set but there is no database to record \
                 submissions into"
            );
            false
        }
        None => {
            info!("signup intake route disabled (GAMESERVERS_SIGNUP_TOKEN unset)");
            false
        }
    };

    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind agent http endpoint on {addr}"))?;
    info!(
        port,
        ingame_route, operator_routes, signup_route, "agent http endpoint listening"
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(async move { shutdown.cancelled().await })
        .await
        .context("agent http endpoint server failed")?;
    Ok(())
}

impl BearerGate {
    pub(crate) fn new(token: Option<String>) -> Self {
        Self {
            expected: token.map(Arc::from),
        }
    }
}

pub(crate) async fn require_token(
    State(gate): State<BearerGate>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    if authorized(gate.expected.as_deref(), request.headers()) {
        next.run(request).await
    } else {
        warn!(
            path = %request.uri().path(),
            "rejected an unauthenticated request to the agent endpoint"
        );
        StatusCode::UNAUTHORIZED.into_response()
    }
}

pub(crate) fn gated(router: Router, token: Option<String>) -> Router {
    let gate = BearerGate::new(token);
    router.layer(from_fn_with_state(gate, require_token))
}

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

fn health(cycles: &crate::cycles::CycleHealth) -> StatusCode {
    let failing = cycles.failing();
    if failing.is_empty() {
        return StatusCode::OK;
    }
    error!(
        ?failing,
        "background cycles are panicking repeatedly; reporting unhealthy so the pod is restarted"
    );
    StatusCode::SERVICE_UNAVAILABLE
}

#[cfg(test)]
#[path = "tests/http.rs"]
mod tests;
