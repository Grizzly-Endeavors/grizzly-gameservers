pub(crate) mod commands;
mod render;

use kube::Client;

/// Per-command state shared with every poise command handler.
pub(crate) struct Data {
    pub(crate) kube_client: Client,
    pub(crate) namespace: String,
    pub(crate) domain: String,
}

pub(crate) type Error = Box<dyn std::error::Error + Send + Sync>;
pub(crate) type Context<'a> = poise::Context<'a, Data, Error>;
