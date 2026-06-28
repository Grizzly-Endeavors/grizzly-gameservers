mod auth;
mod chunking;
pub(crate) mod commands;
pub(crate) mod gary;
mod render;

use std::sync::Arc;

use kube::Client;
use tokio::sync::Mutex;

use crate::agent::{OllamaConfig, SessionStore};
use crate::agones::GameCatalog;

/// Per-command state shared with every poise command handler.
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    /// Client for the in-pod supervisor control API (`/stop`, `/start`, `/restart`).
    pub(crate) http: reqwest::Client,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    /// Port the supervisor's control API listens on inside each game pod.
    pub(crate) control_port: u16,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Serializes the port-lease→Service-create critical section across
    /// concurrent `/create`s so two friends can't claim the same `NodePort`.
    pub(crate) provision_lock: Arc<Mutex<()>>,
    pub(crate) admin_role_id: Option<u64>,
    pub(crate) admin_user_ids: Arc<[u64]>,
    /// Agent ("Gary") model connection, or `None` when no key is configured —
    /// in which case mentions reply that Gary isn't set up.
    pub(crate) ollama: Option<OllamaConfig>,
    /// Short-lived per-`(channel, user)` conversation transcripts giving Gary
    /// follow-up continuity across mentions.
    pub(crate) sessions: Arc<SessionStore>,
}

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Context<'a> = poise::Context<'a, Data, Error>;
