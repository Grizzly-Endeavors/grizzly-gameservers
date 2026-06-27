mod auth;
pub(crate) mod commands;
mod render;

use std::sync::Arc;

use kube::Client;
use tokio::sync::Mutex;

use crate::agones::GameCatalog;

/// Per-command state shared with every poise command handler.
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) catalog: Arc<GameCatalog>,
    /// Serializes the port-lease→Service-create critical section across
    /// concurrent `/create`s so two friends can't claim the same `NodePort`.
    pub(crate) provision_lock: Arc<Mutex<()>>,
    pub(crate) admin_role_id: Option<u64>,
    pub(crate) admin_user_ids: Arc<[u64]>,
}

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Context<'a> = poise::Context<'a, Data, Error>;
