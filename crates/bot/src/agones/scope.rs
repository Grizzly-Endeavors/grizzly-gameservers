use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::{Api, Client};

use super::labels::{GAME_KEY, GUILD_KEY, label_value};
use crate::domain::{GameId, GuildId};

/// Which servers an operation may see or touch — the tenant boundary.
///
/// [`Guild`](ServerScope::Guild) confines discovery and mutation to servers
/// stamped with one Discord guild id. [`All`](ServerScope::All) is the
/// allowlisted cross-guild operator's view: every server, across every guild,
/// including pre-scoping ones that carry no guild label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServerScope {
    All,
    Guild(String),
}

impl ServerScope {
    pub(crate) fn label_selector(&self) -> Option<String> {
        match self {
            Self::All => None,
            Self::Guild(id) => Some(format!("{GUILD_KEY}={id}")),
        }
    }
}

/// Whether a named instance is reachable under a [`ServerScope`], from the
/// caller's point of view. [`Foreign`](ScopeVerdict::Foreign) — the server
/// exists but belongs to another guild — is deliberately surfaced to callers
/// as the same "no such server" message as [`Absent`](ScopeVerdict::Absent), so
/// scoping never leaks the existence of another tenant's servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeVerdict {
    InScope,
    Foreign,
    Absent,
}

pub(crate) async fn verify_scope(
    client: &Client,
    namespace: &str,
    instance: &str,
    scope: &ServerScope,
) -> Result<ScopeVerdict> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance} for scope check"))?
    else {
        return Ok(ScopeVerdict::Absent);
    };
    let guild = label_value(service.metadata.labels.as_ref(), GUILD_KEY);
    Ok(classify(guild, scope))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct InstanceOwner {
    pub(crate) guild: Option<GuildId>,
    pub(crate) game: Option<GameId>,
}

pub(crate) async fn owner_of(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<Option<InstanceOwner>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance} for owner lookup"))?
    else {
        return Ok(None);
    };
    let labels = service.metadata.labels.as_ref();
    Ok(Some(InstanceOwner {
        guild: label_value(labels, GUILD_KEY).map(GuildId::new),
        game: label_value(labels, GAME_KEY).map(GameId::new),
    }))
}

fn classify(guild: Option<&str>, scope: &ServerScope) -> ScopeVerdict {
    match scope {
        ServerScope::All => ScopeVerdict::InScope,
        ServerScope::Guild(id) if guild == Some(id.as_str()) => ScopeVerdict::InScope,
        ServerScope::Guild(_) => ScopeVerdict::Foreign,
    }
}

#[cfg(test)]
#[path = "tests/scope.rs"]
mod tests;
