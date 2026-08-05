//! The in-game agent endpoint: the route the game-pod supervisors POST player
//! `@Gary` chat triggers to. It resolves the server to its guild scope and hands
//! the question to the read-only [`agent`] orchestrator — which answers and
//! broadcasts the reply back into the game over RCON. This is the bot half of
//! the reverse loop; the inbound parsing lives in the supervisor's chat watcher.
//!
//! The route is mounted onto the shared listener in [`crate::http`], which owns
//! the bearer gate that authenticates the caller. It runs alongside the Discord
//! gateway in the same process, sharing Gary's core and session store via
//! [`IngameDeps`] (cloneable handles carved from the command `Data`, the same
//! pattern the backup cycle uses).

mod agent;

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use grizzly_control_api::{INGAME_TRIGGER_PATH, IngameTriggerRequest};
use tokio_util::task::TaskTracker;

use crate::agent::{Environment, OllamaConfig, SessionStore};
use crate::agones::GameCatalog;
use crate::throttle::{TurnSubject, TurnThrottle};

/// Cloneable handles the in-game endpoint shares with the rest of the bot. Every
/// field is a cheap clone (an `Arc` bump or a short owned string), so the endpoint
/// and the Discord shell reach the same cluster client, catalog, and session
/// store without wrapping the whole command `Data` in an `Arc`.
#[derive(Clone)]
pub(crate) struct IngameDeps {
    pub(crate) client: kube::Client,
    pub(crate) http: reqwest::Client,
    /// Dials this server's supervisor (the announce back into chat).
    pub(crate) control_http: crate::agones::ControlClient,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Gary's model connection. `None` means there is nothing to answer with, so
    /// the route isn't mounted at all.
    pub(crate) ollama: Option<OllamaConfig>,
    pub(crate) sessions: Arc<SessionStore>,
    /// DMs the operators when an in-game `@Gary` question exhausts the round
    /// budget, so the escalation reaches a human instead of only a `warn` log.
    pub(crate) notifier: crate::notify::OperatorNotifier,
    /// Which population this process's ordinary traffic belongs to, stamped on
    /// every in-game turn's identity.
    pub(crate) environment: Environment,
    /// Exports every in-game turn to Langfuse, sharing the bot's one exporter.
    pub(crate) telemetry: Arc<crate::telemetry::Telemetry>,
    /// Records an in-game escalation in the product event log. In-game tools are
    /// read-only, so this is the only event the endpoint emits.
    pub(crate) events: Arc<crate::events::EventLog>,
    /// Admission control for paid turns. The in-game surface needs it most: its
    /// callers hold no Discord identity and reach it over a public game port.
    pub(crate) turn_throttle: Arc<TurnThrottle>,
}

impl IngameDeps {
    /// Carve the endpoint's handles out of the command `Data`, so both halves of
    /// the bot share one cluster client, catalog, and session store.
    pub(crate) fn from_data(data: &crate::discord::Data) -> Self {
        Self {
            client: data.kube_client.clone(),
            http: data.http.clone(),
            control_http: data.control_http.clone(),
            namespace: data.namespace.clone(),
            domain: data.domain.clone(),
            control_port: data.control_port,
            catalog: Arc::clone(&data.catalog),
            ollama: data.ollama.clone(),
            sessions: Arc::clone(&data.sessions),
            notifier: data.notifier.clone(),
            environment: data.environment,
            telemetry: Arc::clone(&data.telemetry),
            events: Arc::clone(&data.events),
            turn_throttle: Arc::clone(&data.turn_throttle),
        }
    }
}

/// The route's shared state.
#[derive(Clone)]
struct IngameServer {
    deps: IngameDeps,
    /// Tracks the detached per-trigger answer tasks so the shutdown drain awaits
    /// an in-flight in-game answer instead of dropping it on process exit.
    tasks: TaskTracker,
}

/// The in-game trigger route, gated on the shared bearer token. `token` is
/// `None` when the secret isn't provisioned, which leaves the route open behind
/// the `NetworkPolicy` rather than breaking the reverse loop.
pub(crate) fn router(deps: IngameDeps, token: Option<String>, tasks: TaskTracker) -> Router {
    let state = IngameServer { deps, tasks };
    crate::http::gated(
        Router::new()
            .route(INGAME_TRIGGER_PATH, post(ingame_trigger))
            .with_state(state),
        token,
    )
}

/// Accept a trigger, answer it asynchronously, and return `202` immediately — the
/// reply is delivered back over RCON, so the supervisor's POST never waits on
/// Gary's turn.
async fn ingame_trigger(
    State(server): State<IngameServer>,
    axum::Json(request): axum::Json<IngameTriggerRequest>,
) -> StatusCode {
    let deps = server.deps;
    // The only other brake on this path is a cooldown inside the game pod, which
    // is code the asker is a client of — it paces a polite player and bounds
    // nothing else. This is the authoritative one: the caller needs no Discord
    // identity, so the player name the pod reports is the finest subject there is.
    let subject = TurnSubject::InGamePlayer {
        server: request.server.clone(),
        player: request.player.clone(),
    };
    if deps.turn_throttle.claim(&subject).await.is_err() {
        // Nothing to say back: the reply path is an RCON broadcast into the game's
        // public chat, so answering a flood would spam every player on the server.
        // `claim` has already logged the refusal.
        return StatusCode::TOO_MANY_REQUESTS;
    }
    server.tasks.spawn(async move {
        let Ok(_permit) = deps.turn_throttle.admit_now() else {
            return;
        };
        agent::handle_ingame_question(&deps, &request.server, &request.player, &request.message)
            .await;
    });
    StatusCode::ACCEPTED
}
