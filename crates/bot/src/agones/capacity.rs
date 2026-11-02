//! Reading the namespace's `ResourceQuota` into the shape [`crate::capacity`]
//! reasons about, and pricing what one instance asks of it.
//!
//! Split from the pure module for the usual reason: the arithmetic that decides
//! a refusal is testable without a cluster, and this is the part that needs one.

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, ResourceQuota};
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use super::catalog::GameCatalogEntry;
use super::instance::catalog_sizing;
use crate::capacity::{Admission, Dimension, NamespaceQuota, SDK_SIDECAR};
use crate::events::{parse_bytes, parse_cpu_millicores};
use crate::sizing::Sizing;

/// Read every `ResourceQuota` in the namespace and reduce them to the tightest
/// ceiling on each dimension.
///
/// Several quotas may cover the same namespace, and Kubernetes admits a pod only
/// if *all* of them allow it — so the binding figure per dimension is the one
/// with the least room, not the first one found.
///
/// A namespace with no quota, or one the bot may not read, yields an empty
/// snapshot that constrains nothing. That is the deliberate degradation: the
/// cluster still enforces whatever it enforces, and a bot that cannot see the
/// ceiling must not invent one and start refusing creates that would have
/// succeeded.
///
/// # Errors
///
/// Never — an unreadable quota is logged and reported as unconstrained. The
/// `Result` is kept for symmetry with the other cluster readers and so a future
/// caller that *does* need the failure isn't forced to change the signature.
pub(crate) async fn namespace_quota(client: &Client, namespace: &str) -> Result<NamespaceQuota> {
    let quotas: Api<ResourceQuota> = Api::namespaced(client.clone(), namespace);
    let list = match quotas.list(&ListParams::default()).await {
        Ok(list) => list,
        Err(err) => {
            warn!(
                error = ?err,
                namespace,
                "could not read the namespace quota; capacity checks will not refuse anything"
            );
            return Ok(NamespaceQuota::default());
        }
    };

    let mut snapshot = NamespaceQuota::default();
    for quota in &list.items {
        let Some(status) = quota.status.as_ref() else {
            continue;
        };
        let read = |key: &str, parse: fn(&str) -> Option<i64>| {
            let hard = status.hard.as_ref()?.get(key)?;
            let used = status.used.as_ref()?.get(key)?;
            Some(Dimension {
                hard: parse(&hard.0)?,
                used: parse(&used.0)?,
            })
        };
        let count = |value: &str| value.parse::<i64>().ok();
        tighten(&mut snapshot.pvcs, read("persistentvolumeclaims", count));
        tighten(
            &mut snapshot.storage_bytes,
            read("requests.storage", parse_bytes),
        );
        tighten(
            &mut snapshot.cpu_requests,
            read("requests.cpu", parse_cpu_millicores),
        );
        tighten(
            &mut snapshot.cpu_limits,
            read("limits.cpu", parse_cpu_millicores),
        );
        tighten(
            &mut snapshot.mem_requests,
            read("requests.memory", parse_bytes),
        );
        tighten(&mut snapshot.mem_limits, read("limits.memory", parse_bytes));
    }
    Ok(snapshot)
}

/// Keep whichever of the two dimensions leaves less room, since a pod has to
/// satisfy every quota covering the namespace.
fn tighten(slot: &mut Option<Dimension>, candidate: Option<Dimension>) {
    let Some(candidate) = candidate else {
        return;
    };
    let replace = slot.is_none_or(|held| {
        candidate.hard.saturating_sub(candidate.used) < held.hard.saturating_sub(held.used)
    });
    if replace {
        *slot = Some(candidate);
    }
}

/// What a fresh instance of `entry` will ask the namespace for: its pod (the
/// declared sizing plus the injected SDK sidecar) and the PVC it brings with it.
///
/// `sizing` overrides the catalog's figures for an instance that has been
/// resized. An unreadable catalog template contributes nothing beyond the
/// sidecar — the same deliberately permissive choice `check_create` makes, so a
/// malformed template can't turn into a namespace-wide refusal.
pub(crate) fn create_admission(entry: &GameCatalogEntry, sizing: Option<Sizing>) -> Admission {
    pod_admission(entry, sizing).plus(Admission {
        pvcs: 1,
        storage_bytes: catalog_storage_bytes(entry).unwrap_or(0),
        ..Admission::default()
    })
}

/// What restarting a stopped instance's pod will ask for. No PVC or storage —
/// a cold start reuses the ones the instance already holds.
pub(crate) fn start_admission(entry: &GameCatalogEntry, sizing: Option<Sizing>) -> Admission {
    pod_admission(entry, sizing)
}

fn pod_admission(entry: &GameCatalogEntry, sizing: Option<Sizing>) -> Admission {
    let sizing = sizing.or_else(|| catalog_sizing(entry));
    let game = sizing.map_or_else(Admission::default, |sizing| Admission {
        pvcs: 0,
        storage_bytes: 0,
        cpu_request_millicores: sizing.cpu_request_millicores,
        cpu_limit_millicores: sizing.cpu_limit_millicores,
        mem_request_bytes: sizing.mem_request_bytes,
        mem_limit_bytes: sizing.mem_limit_bytes,
    });
    game.plus(SDK_SIDECAR)
}

/// The storage a game's PVC template claims.
fn catalog_storage_bytes(entry: &GameCatalogEntry) -> Option<i64> {
    let pvc: PersistentVolumeClaim = serde_saphyr::from_str(&entry.pvc_yaml).ok()?;
    let claim = pvc
        .spec?
        .resources?
        .requests?
        .get("storage")
        .map(|quantity| quantity.0.clone())?;
    parse_bytes(&claim)
}

/// Price and check one admission against the namespace's live quota.
///
/// # Errors
///
/// Returns an error only if the cluster cannot be reached; a full namespace is
/// an `Ok(Err(refusal))`, not a fault.
pub(crate) async fn check_namespace_capacity(
    client: &Client,
    namespace: &str,
    adding: Admission,
) -> Result<Result<(), crate::capacity::CapacityRefusal>> {
    let quota = namespace_quota(client, namespace)
        .await
        .with_context(|| format!("failed to read the resource quota for {namespace}"))?;
    Ok(crate::capacity::check_admission(&quota, adding))
}

#[cfg(test)]
#[path = "tests/capacity.rs"]
mod tests;
