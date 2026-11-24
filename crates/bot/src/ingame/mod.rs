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

#[derive(Clone)]
pub(crate) struct IngameDeps {
    pub(crate) client: kube::Client,
    pub(crate) http: reqwest::Client,
    pub(crate) control_http: crate::agones::ControlClient,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    pub(crate) ollama: Option<OllamaConfig>,
    pub(crate) settings: Arc<crate::settings::RuntimeSettings>,
    pub(crate) sessions: Arc<SessionStore>,
    pub(crate) notifier: crate::notify::OperatorNotifier,
    pub(crate) environment: Environment,
    pub(crate) telemetry: Arc<crate::telemetry::Telemetry>,
    pub(crate) events: Arc<crate::events::EventLog>,
    pub(crate) turn_throttle: Arc<TurnThrottle>,
}

impl IngameDeps {
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
            settings: Arc::clone(&data.settings),
            sessions: Arc::clone(&data.sessions),
            notifier: data.notifier.clone(),
            environment: data.environment,
            telemetry: Arc::clone(&data.telemetry),
            events: Arc::clone(&data.events),
            turn_throttle: Arc::clone(&data.turn_throttle),
        }
    }

    pub(crate) fn ollama_now(&self) -> Option<OllamaConfig> {
        crate::discord::ollama_now(self.ollama.as_ref(), &self.settings)
    }
}

#[derive(Clone)]
struct IngameServer {
    deps: IngameDeps,
    tasks: TaskTracker,
}

pub(crate) fn router(deps: IngameDeps, token: Option<String>, tasks: TaskTracker) -> Router {
    let state = IngameServer { deps, tasks };
    crate::http::gated(
        Router::new()
            .route(INGAME_TRIGGER_PATH, post(ingame_trigger))
            .with_state(state),
        token,
    )
}

async fn ingame_trigger(
    State(server): State<IngameServer>,
    axum::Json(request): axum::Json<IngameTriggerRequest>,
) -> StatusCode {
    let deps = server.deps;
    let subject = TurnSubject::InGamePlayer {
        server: request.server.clone(),
        player: request.player.clone(),
    };
    if deps.turn_throttle.claim(&subject).await.is_err() {
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
