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

pub(crate) fn create_admission(entry: &GameCatalogEntry, sizing: Option<Sizing>) -> Admission {
    pod_admission(entry, sizing).plus(Admission {
        pvcs: 1,
        storage_bytes: catalog_storage_bytes(entry).unwrap_or(0),
        ..Admission::default()
    })
}

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
