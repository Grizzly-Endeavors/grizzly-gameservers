use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "agones.dev",
    version = "v1",
    kind = "GameServer",
    namespaced,
    status = "GameServerStatus"
)]
pub(crate) struct GameServerSpec {
    #[serde(default)]
    pub(crate) container: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameServerStatus {
    #[serde(default)]
    pub(crate) state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServerState {
    Ready,
    Allocated,
    Paused,
    Stopping,
    Stopped,
    Unknown,
    Other(String),
}

impl ServerState {
    pub(crate) fn from_agones(state: &str) -> Self {
        match state {
            "Ready" => Self::Ready,
            "Allocated" => Self::Allocated,
            other => Self::Other(other.to_owned()),
        }
    }

    pub(crate) fn is_online(&self) -> bool {
        matches!(self, Self::Ready | Self::Allocated)
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Ready => "Ready",
            Self::Allocated => "Allocated",
            Self::Paused => "Paused",
            Self::Stopping => "Stopping",
            Self::Stopped => "Stopped",
            Self::Unknown => "Unknown",
            Self::Other(state) => state,
        }
    }
}

impl std::fmt::Display for ServerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerSummary {
    pub(crate) name: String,
    pub(crate) game: Option<String>,
    pub(crate) state: ServerState,
    pub(crate) address: Option<String>,
}

pub(crate) fn server_address(instance: &str, domain: &str, node_port: i32) -> String {
    format!("{instance}.{domain}:{node_port}")
}

pub(crate) fn summarize(
    instance: &str,
    game: Option<&str>,
    state: ServerState,
    node_port: Option<i32>,
    domain: &str,
) -> ServerSummary {
    ServerSummary {
        name: instance.to_owned(),
        game: game.map(str::to_owned),
        state,
        address: node_port.map(|port| server_address(instance, domain, port)),
    }
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
