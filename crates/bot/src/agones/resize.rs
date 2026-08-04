//! In-place vertical scaling: change a running game container's CPU and memory
//! without recreating the pod, so nobody playing is disconnected.
//!
//! This is the one place the bot patches a Kubernetes object, and it does so
//! through the `pods/resize` subresource rather than `pods` — that subresource
//! accepts changes to `spec.containers[*].resources` and nothing else, so the
//! grant cannot reach the image, env, volumes or security context (see
//! `cluster/guardrails/bot-rbac.yaml` and ADR-021).
//!
//! The kubelet does the work asynchronously and may decline: shrinking a memory
//! limit below what the container is currently using, or growing beyond what the
//! node has left, is reported back as a pending condition rather than an error on
//! the patch. [`resize_instance`] therefore patches *and then watches*, and
//! reports what actually happened rather than what was asked for.

use std::time::Duration;

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::api::{ListParams, Patch, PatchParams};
use kube::{Api, Client};
use tracing::{debug, warn};

use super::labels::{GAMESERVER_SELECTOR_KEY, is_managed};
use super::provision::{DeletionState, deletion_state};
use super::types::GameServer;
use crate::events::{parse_bytes, parse_cpu_millicores};
use crate::sizing::Sizing;

/// One side (requests or limits) of a container's declared resources.
type Quantities = std::collections::BTreeMap<String, Quantity>;

/// How long to watch for the kubelet to apply a resize before reporting it as
/// still in progress. A CPU-only change lands almost immediately; a memory grow
/// takes a cgroup write. Anything slower is either infeasible (which surfaces as
/// its own condition) or a busy node, and both are better reported than waited on.
const RESIZE_SETTLE_TIMEOUT: Duration = Duration::from_secs(30);
const RESIZE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Pod condition the kubelet sets while a requested resize can't be actioned yet.
const RESIZE_PENDING_CONDITION: &str = "PodResizePending";
/// Pod condition set while the kubelet is actioning an accepted resize.
const RESIZE_IN_PROGRESS_CONDITION: &str = "PodResizeInProgress";
/// `PodResizePending` reason meaning the node cannot satisfy the request as
/// asked — distinct from `Deferred`, which means "not now, but maybe later".
const INFEASIBLE_REASON: &str = "Infeasible";
/// A pod condition only counts when it is actually asserted. Kubernetes may
/// leave a resolved condition in place with `status: "False"` rather than
/// removing it.
const CONDITION_TRUE: &str = "True";

/// What happened to a requested in-place resize.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ResizeOutcome {
    /// The running container now declares the requested sizing.
    Applied,
    /// The kubelet accepted it but hadn't finished within the settle window. The
    /// change is still in flight, not lost.
    InProgress,
    /// The node can't satisfy it as asked — typically shrinking memory below live
    /// usage, or growing past what the node has left. Carries the kubelet's own
    /// message, which names the real obstacle.
    Infeasible(String),
    /// No pod is running, so there is nothing to resize in place; the recorded
    /// override applies at the next start.
    NoPod,
    /// No such instance.
    NotFound,
    /// The instance exists but isn't one of ours.
    NotManaged,
}

/// Apply `sizing` to `instance`'s running game container in place.
///
/// Only the game container is touched: it is named by the `GameServer`'s
/// `spec.container`, and patching by name leaves the Agones SDK sidecar's own
/// footprint alone.
///
/// # Errors
///
/// Returns an error if the cluster can't be reached or the patch is rejected
/// outright (as opposed to accepted-then-declined, which is [`ResizeOutcome::Infeasible`]).
pub(crate) async fn resize_instance(
    client: &Client,
    namespace: &str,
    instance: &str,
    sizing: Sizing,
) -> Result<ResizeOutcome> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    else {
        return Ok(ResizeOutcome::NotFound);
    };
    if !is_managed(gameserver.metadata.labels.as_ref()) {
        return Ok(ResizeOutcome::NotManaged);
    }
    // The catalog names the game container in `spec.container`; without it there
    // is no way to tell the game apart from the injected sidecar, and resizing
    // the wrong one is worse than not resizing.
    let container = gameserver
        .spec
        .container
        .clone()
        .with_context(|| format!("gameserver {instance} does not name its game container"))?;

    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let Some(pod_name) = running_pod_name(&pods, instance).await? else {
        return Ok(ResizeOutcome::NoPod);
    };

    let quantities = sizing.as_quantities();
    let patch = serde_json::json!({
        "spec": {
            "containers": [{
                "name": container,
                "resources": {
                    "requests": {
                        "cpu": quantities.cpu_request,
                        "memory": quantities.mem_request,
                    },
                    "limits": {
                        "cpu": quantities.cpu_limit,
                        "memory": quantities.mem_limit,
                    },
                },
            }],
        }
    });
    pods.patch_subresource(
        "resize",
        &pod_name,
        &PatchParams::default(),
        &Patch::Strategic(patch),
    )
    .await
    .with_context(|| format!("failed to patch resources on pod {pod_name}"))?;
    debug!(instance, pod = %pod_name, container, "requested an in-place resize");

    watch_resize(&pods, &pod_name, &container, sizing).await
}

/// Poll until the container reports the requested sizing, the kubelet declares
/// it infeasible, or the settle window closes.
async fn watch_resize(
    pods: &Api<Pod>,
    pod_name: &str,
    container: &str,
    sizing: Sizing,
) -> Result<ResizeOutcome> {
    let deadline = tokio::time::Instant::now() + RESIZE_SETTLE_TIMEOUT;
    loop {
        let Some(pod) = pods
            .get_opt(pod_name)
            .await
            .with_context(|| format!("failed to re-read pod {pod_name}"))?
        else {
            // The pod went away mid-resize — a crash or an eviction, not a resize
            // failure. The override is recorded, so the replacement gets it.
            warn!(pod = pod_name, "pod disappeared while resizing");
            return Ok(ResizeOutcome::NoPod);
        };
        if let Some(message) = infeasible_message(&pod) {
            return Ok(ResizeOutcome::Infeasible(message));
        }
        if applied(&pod, container, sizing) {
            return Ok(ResizeOutcome::Applied);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(ResizeOutcome::InProgress);
        }
        tokio::time::sleep(RESIZE_POLL_INTERVAL).await;
    }
}

/// The kubelet's explanation when it has decided it cannot satisfy the request.
/// `Deferred` is deliberately not treated as infeasible — it means the node is
/// full *now*, and the resize stays queued.
fn infeasible_message(pod: &Pod) -> Option<String> {
    let condition = pod
        .status
        .as_ref()?
        .conditions
        .as_ref()?
        .iter()
        .find(|condition| condition.type_ == RESIZE_PENDING_CONDITION)?;
    // Status, not mere presence: the kubelet may clear a resolved condition by
    // flipping it to False rather than removing it, and treating a leftover
    // False as live would strand every subsequent resize on this pod.
    if condition.status != CONDITION_TRUE {
        return None;
    }
    if condition.reason.as_deref() != Some(INFEASIBLE_REASON) {
        return None;
    }
    Some(
        condition
            .message
            .clone()
            .unwrap_or_else(|| "the node can't fit the requested size".to_owned()),
    )
}

/// Whether the running container has actually taken the new sizing.
///
/// Read from `status.containerStatuses[].resources` — what the container is
/// *running* with — never from the spec, which reflects the request the moment
/// the patch is accepted and would report success before anything happened.
fn applied(pod: &Pod, container: &str, sizing: Sizing) -> bool {
    if in_progress(pod) {
        return false;
    }
    let Some(status) = pod
        .status
        .as_ref()
        .and_then(|status| status.container_statuses.as_ref())
        .and_then(|statuses| statuses.iter().find(|status| status.name == container))
    else {
        return false;
    };
    let Some(resources) = status.resources.as_ref() else {
        return false;
    };
    // Compared as parsed numbers, not strings: Kubernetes normalizes quantities
    // on the way through ("2000m" comes back as "2"), so a string comparison
    // would report a successful resize as still pending forever.
    let cpu = |side: Option<&Quantities>| {
        side.and_then(|map| map.get("cpu"))
            .and_then(|quantity| parse_cpu_millicores(&quantity.0))
    };
    let memory = |side: Option<&Quantities>| {
        side.and_then(|map| map.get("memory"))
            .and_then(|quantity| parse_bytes(&quantity.0))
    };
    cpu(resources.requests.as_ref()) == Some(sizing.cpu_request_millicores)
        && cpu(resources.limits.as_ref()) == Some(sizing.cpu_limit_millicores)
        && memory(resources.requests.as_ref()) == Some(sizing.mem_request_bytes)
        && memory(resources.limits.as_ref()) == Some(sizing.mem_limit_bytes)
}

/// Whether the kubelet says it is still working on a resize.
fn in_progress(pod: &Pod) -> bool {
    pod.status
        .as_ref()
        .and_then(|status| status.conditions.as_ref())
        .is_some_and(|conditions| {
            conditions.iter().any(|condition| {
                condition.type_ == RESIZE_IN_PROGRESS_CONDITION
                    && condition.status == CONDITION_TRUE
            })
        })
}

/// The name of the instance's live pod, skipping one on its way out — the same
/// rule the supervisor client uses to avoid dialling a dead address.
async fn running_pod_name(pods: &Api<Pod>, instance: &str) -> Result<Option<String>> {
    let selector = format!("{GAMESERVER_SELECTOR_KEY}={instance}");
    let list = pods
        .list(&ListParams::default().labels(&selector))
        .await
        .with_context(|| format!("failed to list pods for gameserver {instance}"))?;
    Ok(list.items.into_iter().find_map(running_name))
}

/// The name of a pod that is genuinely live: `Running` and not already being
/// evicted. A pod on a dead node keeps reporting `Running` forever, and resizing
/// one that is on its way out would silently do nothing.
fn running_name(pod: Pod) -> Option<String> {
    if deletion_state(&pod.metadata) == DeletionState::Terminating {
        return None;
    }
    if pod.status.as_ref()?.phase.as_deref() != Some("Running") {
        return None;
    }
    pod.metadata.name
}

#[cfg(test)]
#[path = "tests/resize.rs"]
mod tests;
