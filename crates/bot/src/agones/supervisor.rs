use std::time::Duration;

use anyhow::{Context, Result};
use grizzly_control_api::{ControlCommand, ControlOk, ResultKind};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::warn;

use super::labels::{GAMESERVER_SELECTOR_KEY, is_managed};
use super::types::GameServer;

/// Per-request timeout for the mutating supervisor calls. `/stop` and `/restart`
/// reply only after the in-pod graceful stop finishes (Minecraft's SIGTERM
/// world-save), so this must exceed the supervisor's `SUPERVISOR_GRACEFUL_TIMEOUT_SECS`
/// (90s default) plus respawn margin — otherwise the bot times out and reports a
/// failure while the restart actually succeeds. The client's short default still
/// applies to cheap calls; this overrides it only where the work is genuinely slow.
const CONTROL_MUTATION_TIMEOUT: Duration = Duration::from_secs(120);

/// Where an instance is in the warm/cold spectrum, used to route `/start`:
/// a live pod takes the fast supervisor path; a killed one needs a reschedule.
pub(crate) enum RuntimeState {
    /// The `GameServer` (and its pod) exist — the supervisor can act in place.
    PodUp,
    /// No `GameServer`, but the Service survives — `/kill`ed, needs a cold start.
    Killed,
    /// Nothing by that name.
    Absent,
}

/// Result of asking the in-pod supervisor to change the game process state.
/// Distinguishes the happy paths (so the friend-facing reply is accurate) from
/// the ways the request can't be served.
pub(crate) enum SupervisorOutcome {
    /// `/stop` succeeded: process down, pod kept warm.
    Paused,
    /// `/start` succeeded: process relaunching in place.
    Resumed,
    /// `/restart` succeeded: process bounced in place.
    Restarted,
    /// `/stop` on an already-paused server.
    AlreadyStopped,
    /// `/start` on an already-running server.
    AlreadyRunning,
    /// The pod exists but isn't far enough along to have a reachable control API.
    PodNotReady,
    /// The control API couldn't be reached or returned an error.
    Unreachable,
    /// No live `GameServer` by that name.
    NotFound,
    /// The instance exists but isn't shim-managed.
    NotManaged,
}

/// Pause the game process in place (keep the pod).
///
/// # Errors
///
/// Returns an error if the cluster can't be queried for the target pod.
pub(crate) async fn supervisor_stop(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<SupervisorOutcome> {
    supervisor_action(
        client,
        http,
        namespace,
        instance,
        control_port,
        ControlCommand::Stop,
    )
    .await
}

/// Start the game process in place on a still-running pod (the warm `/start`).
///
/// # Errors
///
/// Returns an error if the cluster can't be queried for the target pod.
pub(crate) async fn supervisor_start(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<SupervisorOutcome> {
    supervisor_action(
        client,
        http,
        namespace,
        instance,
        control_port,
        ControlCommand::Start,
    )
    .await
}

/// Bounce the game process in place.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried for the target pod.
pub(crate) async fn supervisor_restart(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<SupervisorOutcome> {
    supervisor_action(
        client,
        http,
        namespace,
        instance,
        control_port,
        ControlCommand::Restart,
    )
    .await
}

/// Classify an instance as warm (pod up), cold (killed), or absent so `/start`
/// can pick the fast or the reschedule path.
///
/// # Errors
///
/// Returns an error if the gameservers or services can't be listed.
pub(crate) async fn instance_runtime_state(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<RuntimeState> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    if gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
        .is_some()
    {
        return Ok(RuntimeState::PodUp);
    }
    let services: Api<Service> = Api::namespaced(client.clone(), namespace);
    if services
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read service {instance}"))?
        .is_some()
    {
        return Ok(RuntimeState::Killed);
    }
    Ok(RuntimeState::Absent)
}

async fn supervisor_action(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    command: ControlCommand,
) -> Result<SupervisorOutcome> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    else {
        return Ok(SupervisorOutcome::NotFound);
    };
    if !is_managed(gameserver.metadata.labels.as_ref()) {
        return Ok(SupervisorOutcome::NotManaged);
    }

    let Some(pod_ip) = gameserver_pod_ip(client, namespace, instance).await? else {
        return Ok(SupervisorOutcome::PodNotReady);
    };

    let url = format!("http://{pod_ip}:{control_port}{}", command.path());
    match http
        .post(&url)
        .timeout(CONTROL_MUTATION_TIMEOUT)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            let ok: ControlOk = response
                .json()
                .await
                .with_context(|| format!("failed to parse supervisor reply from {url}"))?;
            Ok(map_result_kind(ok.result))
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            warn!(%status, body, url, "supervisor control api returned an error");
            Ok(SupervisorOutcome::Unreachable)
        }
        Err(err) => {
            warn!(error = ?err, url, "failed to reach supervisor control api");
            Ok(SupervisorOutcome::Unreachable)
        }
    }
}

/// Resolve the pod IP for an instance via the `agones.dev/gameserver` label
/// Agones stamps on each game-server pod, picking a Running pod.
///
/// # Errors
///
/// Returns an error if pods can't be listed from the Kubernetes API.
async fn gameserver_pod_ip(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<Option<String>> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let selector = format!("{GAMESERVER_SELECTOR_KEY}={instance}");
    let list = pods
        .list(&ListParams::default().labels(&selector))
        .await
        .with_context(|| format!("failed to list pods for gameserver {instance}"))?;
    Ok(list.items.iter().find_map(running_pod_ip))
}

/// The pod IP of a Running pod, or `None` if it isn't running or has no IP yet.
fn running_pod_ip(pod: &Pod) -> Option<String> {
    let status = pod.status.as_ref()?;
    if status.phase.as_deref() != Some("Running") {
        return None;
    }
    status.pod_ip.clone()
}

fn map_result_kind(kind: ResultKind) -> SupervisorOutcome {
    match kind {
        ResultKind::Stopping => SupervisorOutcome::Paused,
        ResultKind::AlreadyStopped => SupervisorOutcome::AlreadyStopped,
        ResultKind::Starting => SupervisorOutcome::Resumed,
        ResultKind::AlreadyRunning => SupervisorOutcome::AlreadyRunning,
        ResultKind::Restarting => SupervisorOutcome::Restarted,
    }
}

#[cfg(test)]
#[path = "tests/supervisor.rs"]
mod tests;
