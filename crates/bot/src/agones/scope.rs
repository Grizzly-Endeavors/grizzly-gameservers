use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Service;
use kube::{Api, Client};

use super::labels::{GUILD_KEY, label_value};

/// Which servers an operation may see or touch — the tenant boundary.
///
/// [`Guild`](ServerScope::Guild) confines discovery and mutation to servers
/// stamped with one Discord guild id. [`All`](ServerScope::All) is the
/// allowlisted cross-guild operator's view: every server, across every guild,
/// including pre-scoping ones that carry no guild label.
#[derive(Clone, Debug)]
pub(crate) enum ServerScope {
    All,
    Guild(String),
}

impl ServerScope {
    /// The Kubernetes label selector that restricts a `list` to this scope, or
    /// `None` for [`All`](ServerScope::All) (no filter — list everything).
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

/// Decide whether `instance` is visible to `scope`, reading the guild label
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
    let guild = label_value(service.metadata.labels.as_ref(), GUILD_KEY);
    Ok(classify(guild, scope))
}

/// The Discord guild id that owns `instance`, read off its Service's guild
/// label — or `None` when the server doesn't exist or carries no guild label
/// (a pre-scoping or platform-managed object). Used by the in-game entrypoint,
/// which has only a server name and must derive the guild scope from it.
///
/// # Errors
///
/// Returns an error if the Service cannot be read from the Kubernetes API.
pub(crate) async fn guild_of(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<Option<String>> {
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    let Some(service) = services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance} for guild lookup"))?
    else {
        return Ok(None);
    };
    Ok(label_value(service.metadata.labels.as_ref(), GUILD_KEY).map(str::to_owned))
}

/// Decide the verdict for an instance whose owning guild label is `guild`
/// (`None` when the label is absent — a pre-scoping or Flux-managed object).
/// Pure so the tenancy policy is unit-tested without a live cluster.
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
