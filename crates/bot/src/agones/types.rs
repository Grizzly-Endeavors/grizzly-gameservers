use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Minimal typed view of the Agones `GameServer` custom resource. Only the
/// fields the bot reads are modelled; serde ignores the rest of the spec and
/// status, so this stays decoupled from Agones' full schema.
#[derive(CustomResource, Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "agones.dev",
    version = "v1",
    kind = "GameServer",
    namespaced,
    status = "GameServerStatus"
)]
pub(crate) struct GameServerSpec {
    /// Name of the container that owns the game port. Unused by the listing,
    /// present so the spec type is non-empty and round-trips cleanly.
    #[serde(default)]
    pub(crate) container: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GameServerStatus {
    /// Agones lifecycle state (`Scheduled`, `Ready`, `Allocated`, ...).
    #[serde(default)]
    pub(crate) state: Option<String>,
}

/// Friend-facing summary of one game server: its name, the catalog game it runs
/// (absent only if the label is somehow missing), current Agones state, and the
/// address to connect to (absent when no `NodePort` is exposed yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ServerSummary {
    pub(crate) name: String,
    pub(crate) game: Option<String>,
    pub(crate) state: String,
    pub(crate) address: Option<String>,
}

/// Compose the friend-facing connection address `<name>.<domain>:<node_port>`.
/// Single definition shared by the lister and the provisioner.
pub(crate) fn server_address(instance: &str, domain: &str, node_port: i32) -> String {
    format!("{instance}.{domain}:{node_port}")
}

/// Build a [`ServerSummary`], composing the connection address as
/// `<name>.<domain>:<node_port>` when a `NodePort` was resolved for the server.
pub(crate) fn summarize(
    instance: &str,
    game: Option<&str>,
    state: Option<&str>,
    node_port: Option<i32>,
    domain: &str,
) -> ServerSummary {
    ServerSummary {
        name: instance.to_owned(),
        game: game.map(str::to_owned),
        state: state.unwrap_or("Unknown").to_owned(),
        address: node_port.map(|port| server_address(instance, domain, port)),
    }
}

#[cfg(test)]
#[path = "tests/types.rs"]
mod tests;
