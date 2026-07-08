use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::{Api, Client};

use super::labels::CHANNEL_KEY;

/// Which servers an operation may see or touch — the tenant boundary.
///
/// [`Channel`](ServerScope::Channel) confines discovery and mutation to servers
/// stamped with one Discord channel id (a DM being its own channel). [`All`](
/// ServerScope::All) is the allowlisted super-admin's cross-channel view: every
/// server, including pre-scoping ones that carry no channel label.
#[derive(Clone, Debug)]
pub(crate) enum ServerScope {
    All,
    Channel(String),
}

impl ServerScope {
    /// The Kubernetes label selector that restricts a `list` to this scope, or
    /// `None` for [`All`](ServerScope::All) (no filter — list everything).
    pub(crate) fn label_selector(&self) -> Option<String> {
        match self {
            Self::All => None,
            Self::Channel(id) => Some(format!("{CHANNEL_KEY}={id}")),
        }
    }
}

/// Whether a named instance is reachable under a [`ServerScope`], from the
/// caller's point of view. [`Foreign`](ScopeVerdict::Foreign) — the server
/// exists but belongs to another channel — is deliberately surfaced to callers
/// as the same "no such server" message as [`Absent`](ScopeVerdict::Absent), so
/// scoping never leaks the existence of another tenant's servers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeVerdict {
    InScope,
    Foreign,
    Absent,
}

/// Decide whether `instance` is visible to `scope`, reading the channel label
/// off the instance's Service — the one object of the trio that survives both
/// `/stop` and `/shutdown`, so the verdict holds in every lifecycle state.
///
/// A managed-ness check is intentionally left to the downstream lifecycle call
/// (which reports `NotManaged` itself); this only answers the tenancy question.
///
/// # Errors
///
/// Returns an error if the Service cannot be read from the Kubernetes API.
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
    let channel = service
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(CHANNEL_KEY))
        .map(String::as_str);
    Ok(classify(channel, scope))
}

/// The Discord channel id that owns `instance`, read off its Service's channel
/// label — or `None` when the server doesn't exist or carries no channel label
/// (a pre-scoping or platform-managed object). Used by the in-game entrypoint,
/// which has only a server name and must derive the channel scope from it.
///
/// # Errors
///
/// Returns an error if the Service cannot be read from the Kubernetes API.
pub(crate) async fn channel_of(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<Option<String>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance} for channel lookup"))?
    else {
        return Ok(None);
    };
    Ok(service
        .metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(CHANNEL_KEY))
        .cloned())
}

/// Decide the verdict for an instance whose owning channel label is `channel`
/// (`None` when the label is absent — a pre-scoping or Flux-managed object).
/// Pure so the tenancy policy is unit-tested without a live cluster.
fn classify(channel: Option<&str>, scope: &ServerScope) -> ScopeVerdict {
    match scope {
        ServerScope::All => ScopeVerdict::InScope,
        ServerScope::Channel(id) if channel == Some(id.as_str()) => ScopeVerdict::InScope,
        ServerScope::Channel(_) => ScopeVerdict::Foreign,
    }
}

#[cfg(test)]
#[path = "tests/scope.rs"]
mod tests;
