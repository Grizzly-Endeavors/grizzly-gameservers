use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{Api, DeleteParams, ListParams};
use kube::{Client, Error as KubeError};
use tracing::{error, info, warn};

use super::catalog::GameCatalog;
use super::labels::{GAMESERVER_SELECTOR_KEY, is_managed, label_value};
use super::provision::{
    DeleteWait, DeletionState, StartBegin, begin_start, delete_and_confirm, deletion_state,
    gameserver_api,
};
use crate::domain::InstanceName;
use crate::events::{Attribution, EventKind, ProductEvent, mapping};

pub(crate) struct StrandedCtx<'a> {
    pub(crate) client: &'a Client,
    pub(crate) namespace: &'a str,
    pub(crate) domain: &'a str,
    pub(crate) catalog: &'a GameCatalog,
    pub(crate) events: &'a crate::events::EventLog,
    pub(crate) sizing: &'a crate::sizing::InstanceSizing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recovery {
    Recycle,
    Release,
}

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

async fn stranded_instances(
    ctx: &StrandedCtx<'_>,
    dwell: Duration,
) -> Result<Vec<(String, String)>> {
    let pods: Api<Pod> = Api::namespaced(ctx.client.clone(), ctx.namespace);
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

async fn recover_instance(ctx: &StrandedCtx<'_>, instance: &str, pod: &str) -> Result<()> {
    let gameservers = gameserver_api(ctx.client, ctx.namespace);
    let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    else {
        force_delete_pod(ctx, pod).await?;
        return Ok(());
    };
    if !is_managed(gameserver.metadata.labels.as_ref()) {
        return Ok(());
    }

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
            ctx.client,
            ctx.namespace,
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

fn terminating_beyond(meta: &ObjectMeta, now_unix_secs: i64, dwell: Duration) -> bool {
    let Some(deleted_at) = meta.deletion_timestamp.as_ref() else {
        return false;
    };
    let dwell_secs = i64::try_from(dwell.as_secs()).unwrap_or(i64::MAX);
    now_unix_secs.saturating_sub(deleted_at.0.as_second()) >= dwell_secs
}

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
