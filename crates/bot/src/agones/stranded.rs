//! Recovery for instances stranded by a node that stopped answering.
//!
//! When a node dies, Kubernetes evicts its pods, but those deletes can never
//! finish: with no kubelet left to confirm the kill, each pod hangs in
//! Terminating indefinitely. Agones won't release a `GameServer`'s finalizer
//! while its pod still exists, so the instance stays pinned to the dead node
//! until it comes back — nothing reschedules it, and no lifecycle command can
//! move it, because every path either needs a live pod or needs the `GameServer`
//! gone.
//!
//! One force-delete of the pod breaks that: Agones sees the pod disappear,
//! releases the finalizer, and the instance can be recreated wherever there's
//! room. That is the only reason the bot holds `pods: delete` in its namespace.
//!
//! It deliberately does **not** touch nodes. Tainting the node
//! `node.kubernetes.io/out-of-service` is the Kubernetes-sanctioned route and
//! also detaches volumes properly, but it needs cluster-scoped node patch, and
//! the whole point of the guardrails is that this workload's blast radius stops
//! at its own namespace (`docs/design/00-overview.md`). A pod still Terminating
//! long past its grace period carries the same information — a kubelet that is
//! answering finishes a delete inside that window — without leaving the
//! namespace to find it.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams};
use kube::{Client, Error as KubeError};
use tracing::{error, info, warn};

use super::catalog::GameCatalog;
use super::directory::Servers;
use super::labels::{GAMESERVER_SELECTOR_KEY, is_managed, label_value};
use super::provision::{
    DeleteWait, DeletionState, StartBegin, begin_start, delete_and_confirm, deletion_state,
    gameserver_api,
};
use crate::domain::InstanceName;
use crate::events::{Attribution, EventKind, ProductEvent, mapping};

/// Cluster and catalog handles one reconcile pass needs.
pub(crate) struct StrandedCtx<'a> {
    pub(crate) client: &'a Client,
    pub(crate) namespace: &'a str,
    pub(crate) control_port: u16,
    pub(crate) domain: &'a str,
    pub(crate) catalog: &'a GameCatalog,
    /// Records a recycle as a system-driven start. Strand-and-recycle frequency
    /// is the only crash signal nobody is around to report.
    pub(crate) events: &'a crate::events::EventLog,
    /// Per-instance resource overrides, so a recycled instance comes back the
    /// size it was rather than the size its catalog entry declares.
    pub(crate) sizing: &'a crate::sizing::InstanceSizing,
}

/// What clearing a stranded instance should leave behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    /// The `GameServer` was intact when the node died, so the instance was meant
    /// to be running: bring it back once the pod is out of the way.
    Recycle,
    /// The `GameServer` was already being deleted, so someone asked for this to
    /// stop. Let the teardown finish and leave it down.
    Release,
}

/// Force out every pod that has been stuck terminating longer than `dwell`, and
/// bring back the instances that were running when their node stopped
/// answering.
///
/// Never returns an error: a reconcile pass runs unattended on a timer, so a bad
/// pass is logged and the next one retries rather than killing the task.
pub(crate) async fn run_stranded_cycle(ctx: &StrandedCtx<'_>, dwell: Duration) {
    let stranded = match stranded_instances(ctx, dwell).await {
        Ok(stranded) => stranded,
        Err(err) => {
            error!(error = ?err, "failed to scan for stranded servers");
            return;
        }
    };
    if stranded.is_empty() {
        return;
    }

    for (instance, pod) in stranded {
        if let Err(err) = recover_instance(ctx, &instance, &pod).await {
            error!(
                error = ?err,
                instance,
                "failed to recover a stranded server; will retry next cycle"
            );
        }
    }
}

/// The managed instances whose pod has been terminating past `dwell`, paired
/// with the name of the pod holding them there.
async fn stranded_instances(
    ctx: &StrandedCtx<'_>,
    dwell: Duration,
) -> Result<Vec<(String, String)>> {
    let pods: Api<Pod> = Api::namespaced(ctx.client.clone(), ctx.namespace);
    // Existence selector: every Agones game-server pod carries this key with its
    // GameServer's name, so one list covers the namespace's game pods and skips
    // the bot's own.
    let list = pods
        .list(&ListParams::default().labels(GAMESERVER_SELECTOR_KEY))
        .await
        .with_context(|| format!("failed to list game pods in namespace {}", ctx.namespace))?;

    let now = unix_seconds_now();
    let mut stranded = Vec::new();
    for pod in &list.items {
        if !terminating_beyond(&pod.metadata, now, dwell) {
            continue;
        }
        let (Some(instance), Some(pod_name)) = (
            label_value(pod.metadata.labels.as_ref(), GAMESERVER_SELECTOR_KEY),
            pod.metadata.name.as_deref(),
        ) else {
            continue;
        };
        stranded.push((instance.to_owned(), pod_name.to_owned()));
    }
    Ok(stranded)
}

/// Force one instance's stranded pod out, then either bring the instance back or
/// let its teardown complete.
async fn recover_instance(ctx: &StrandedCtx<'_>, instance: &str, pod: &str) -> Result<()> {
    let gameservers = gameserver_api(ctx.client, ctx.namespace);
    let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    else {
        // No GameServer left to reason about; the pod is debris from a teardown
        // that already finished everywhere else.
        force_delete_pod(ctx, pod).await?;
        return Ok(());
    };
    if !is_managed(gameserver.metadata.labels.as_ref()) {
        return Ok(());
    }

    // Read the intent *before* touching anything: an intact GameServer means the
    // instance was running when its node went away, while one already carrying a
    // deletion timestamp means a shutdown was asked for and should be honoured.
    let recovery = match deletion_state(&gameserver.metadata) {
        DeletionState::Live => Recovery::Recycle,
        DeletionState::Terminating => Recovery::Release,
    };
    warn!(
        instance,
        pod,
        ?recovery,
        "server stranded on an unresponsive node; forcing its pod out"
    );

    force_delete_pod(ctx, pod).await?;
    if matches!(
        delete_and_confirm(&gameservers, instance).await?,
        DeleteWait::Unconfirmed
    ) {
        // The finalizer outlived the pod delete, so something else is holding it.
        // Leave it for the next pass rather than recreating against a live name.
        warn!(
            instance,
            "gameserver still held after its pod was forced out; retrying next cycle"
        );
        return Ok(());
    }

    if recovery == Recovery::Release {
        info!(instance, "stranded server released; leaving it shut down");
        return Ok(());
    }
    recycle_instance(ctx, instance).await
}

/// Recreate a cleared instance's `GameServer` so the scheduler places it on a
/// node that is actually answering. Its Service and PVC survived, so the world
/// and the leased port come back with it.
async fn recycle_instance(ctx: &StrandedCtx<'_>, instance: &str) -> Result<()> {
    let outcome = begin_start(
        ctx.client,
        ctx.namespace,
        ctx.domain,
        ctx.catalog,
        instance,
        ctx.sizing.get(instance).await,
    )
    .await?;
    ctx.events
        .record_lifecycle(
            Servers::cluster(ctx.client, ctx.namespace, ctx.control_port),
            ProductEvent::new(
                EventKind::ServerStarted,
                &Attribution::system(),
                mapping::start_begin(&outcome),
            )
            .instance(InstanceName::new(instance)),
        )
        .await;
    match outcome {
        StartBegin::Starting { .. } | StartBegin::AlreadyRunning => {
            info!(instance, "stranded server recycled onto a healthy node");
        }
        // The Service is the instance's durable record; without it there is
        // nothing to rebuild from and nothing the next pass could do differently.
        StartBegin::NotFound => {
            warn!(
                instance,
                "stranded server has no service left to rebuild from"
            );
        }
        StartBegin::NotManaged => {
            warn!(instance, "stranded server is not shim-managed; left alone");
        }
        StartBegin::UnknownGame(game) => {
            warn!(
                instance,
                game, "stranded server's game left the catalog; left shut down"
            );
        }
        // Left shut down on purpose rather than retried into a full namespace:
        // the next reconcile pass tries again, and by then something may have
        // freed room.
        StartBegin::ClusterFull(refusal) => {
            warn!(
                instance,
                refusal = %refusal.summary(),
                "no room to recycle the stranded server; leaving it for the next pass"
            );
        }
    }
    Ok(())
}

/// Delete a pod with no grace period, tolerating one that has already gone.
///
/// The pod is already past its graceful stop — it was evicted a dwell window ago
/// and its node never answered — so there is no world-save left to wait for.
async fn force_delete_pod(ctx: &StrandedCtx<'_>, pod: &str) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(ctx.client.clone(), ctx.namespace);
    match pods
        .delete(pod, &DeleteParams::default().grace_period(0))
        .await
    {
        Ok(_) => Ok(()),
        Err(KubeError::Api(response)) if response.code == 404 => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to force-delete pod {pod}")),
    }
}

/// Whether an object has been terminating for at least `dwell`.
///
/// `now` is passed in rather than read here so the decision stays a pure
/// function of cluster state and the clock.
fn terminating_beyond(meta: &ObjectMeta, now_unix_secs: i64, dwell: Duration) -> bool {
    let Some(deleted_at) = meta.deletion_timestamp.as_ref() else {
        return false;
    };
    let dwell_secs = i64::try_from(dwell.as_secs()).unwrap_or(i64::MAX);
    now_unix_secs.saturating_sub(deleted_at.0.as_second()) >= dwell_secs
}

/// Wall-clock now as Unix seconds. Before the epoch is not a time this can be
/// running, so a clock that claims otherwise reads as "no time has passed" and
/// simply defers every recovery to a later pass.
fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "tests/stranded.rs"]
mod tests;
