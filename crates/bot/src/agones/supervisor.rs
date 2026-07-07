use std::time::Duration;

use anyhow::{Context, Result};
use grizzly_control_api::{
    ControlCommand, ControlError, ControlOk, ProcessPhase, ResultKind, StatusResponse,
};
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
const CONTROL_MUTATION_TIMEOUT: Duration = Duration::from_mins(2);

/// Per-request timeout for the cheap `GET /status` poll. Touches only in-memory
/// state, so a slower reply means a wedged pod, not real work.
const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// How long [`wait_for_ready`] polls for a (re)starting server to come back up
/// before giving up. Matches the create/start readiness budget so a slow first
/// boot (world generation) has the same room here as it does there.
const READY_WAIT_TIMEOUT: Duration = Duration::from_mins(5);
const READY_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// Where an instance is in the warm/cold spectrum, used to route `/start`:
/// a live pod takes the fast supervisor path; a shut-down one needs a reschedule.
pub(crate) enum RuntimeState {
    /// The `GameServer` (and its pod) exist — the supervisor can act in place.
    PodUp,
    /// No `GameServer`, but the Service survives — `/shutdown`ed, needs a cold start.
    Down,
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
    /// The control API couldn't be reached, or replied with a body we couldn't
    /// parse as a [`ControlError`].
    Unreachable,
    /// The supervisor was reached but refused or failed the request; carries
    /// its developer-facing message for the caller to relay.
    Failed(String),
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

/// Classify an instance as warm (pod up), cold (shut down), or absent so `/start`
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
        return Ok(RuntimeState::Down);
    }
    Ok(RuntimeState::Absent)
}

/// Where a managed instance's in-pod control API is, or why it can't be reached.
/// Shared by the lifecycle actions here and the filesystem client so both run the
/// same existence/managed/pod-IP gate before issuing an HTTP request.
pub(super) enum PodTarget {
    /// The pod is up with an IP; its control API is at this address.
    Ready(String),
    /// No `GameServer` by that name.
    NotFound,
    /// The instance exists but isn't shim-managed.
    NotManaged,
    /// The pod isn't far enough along to have a reachable control API.
    PodNotReady,
}

/// Validate that `instance` is a shim-managed `GameServer` and resolve its pod IP.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried for the gameserver or pod.
pub(super) async fn resolve_managed_pod(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<PodTarget> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    else {
        return Ok(PodTarget::NotFound);
    };
    if !is_managed(gameserver.metadata.labels.as_ref()) {
        return Ok(PodTarget::NotManaged);
    }
    match gameserver_pod_ip(client, namespace, instance).await? {
        Some(pod_ip) => Ok(PodTarget::Ready(pod_ip)),
        None => Ok(PodTarget::PodNotReady),
    }
}

async fn supervisor_action(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    command: ControlCommand,
) -> Result<SupervisorOutcome> {
    let pod_ip = match resolve_managed_pod(client, namespace, instance).await? {
        PodTarget::Ready(pod_ip) => pod_ip,
        PodTarget::NotFound => return Ok(SupervisorOutcome::NotFound),
        PodTarget::NotManaged => return Ok(SupervisorOutcome::NotManaged),
        PodTarget::PodNotReady => return Ok(SupervisorOutcome::PodNotReady),
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
            match response.json::<ControlError>().await {
                Ok(error) => {
                    warn!(%status, error = error.error, url, "supervisor control api refused the request");
                    Ok(SupervisorOutcome::Failed(error.error))
                }
                Err(err) => {
                    warn!(%status, error = ?err, url, "supervisor control api returned an unreadable error");
                    Ok(SupervisorOutcome::Unreachable)
                }
            }
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

/// How a wait for a (re)starting server to come back up resolved. Every variant
/// is terminal for the wait: the caller reports it and does not keep polling.
pub(crate) enum ReadyWait {
    /// The game process is accepting connections again.
    Ready,
    /// The process crashed while coming up (a bad change is the usual cause).
    Crashed,
    /// The server is paused, so it will never come up on its own.
    Stopped,
    /// The wait budget elapsed while the server was still coming up.
    TimedOut,
    /// No live `GameServer` by that name.
    NotFound,
    /// The instance exists but isn't shim-managed.
    NotManaged,
}

/// One `GET /status` poll: the parsed body, or why it couldn't be read this tick.
/// The transient reasons ([`Self::PodNotReady`], [`Self::Unreachable`]) are
/// expected during a cold start and mean "keep polling", not "give up".
enum StatusPoll {
    Ok(StatusResponse),
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable,
}

/// Block until a starting or restarting server is actually accepting players
/// again, so the caller can wait once instead of churning on repeated status
/// checks. Polls the supervisor's honest per-boot [`ProcessPhase`]: `Running`
/// means up *now* (a warm relaunch reads `Starting` until the new child re-binds),
/// so this doesn't return early on a restart. Gives up after [`READY_WAIT_TIMEOUT`].
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn wait_for_ready(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<ReadyWait> {
    let deadline = tokio::time::Instant::now() + READY_WAIT_TIMEOUT;
    let mut ticker = tokio::time::interval(READY_POLL_INTERVAL);

    loop {
        ticker.tick().await;
        match poll_status(client, http, namespace, instance, control_port).await? {
            StatusPoll::Ok(status) => match status.process {
                ProcessPhase::Running => return Ok(ReadyWait::Ready),
                ProcessPhase::Crashed => return Ok(ReadyWait::Crashed),
                ProcessPhase::Stopped => return Ok(ReadyWait::Stopped),
                // Still coming up (or briefly bouncing) — keep waiting.
                ProcessPhase::Starting | ProcessPhase::Stopping => {}
            },
            StatusPoll::NotFound => return Ok(ReadyWait::NotFound),
            StatusPoll::NotManaged => return Ok(ReadyWait::NotManaged),
            // Transient while a cold-started pod is still scheduling — keep waiting.
            StatusPoll::PodNotReady | StatusPoll::Unreachable => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(ReadyWait::TimedOut);
        }
    }
}

/// Fetch and parse one `GET /status` from the instance's supervisor.
async fn poll_status(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<StatusPoll> {
    let pod_ip = match resolve_managed_pod(client, namespace, instance).await? {
        PodTarget::Ready(pod_ip) => pod_ip,
        PodTarget::NotFound => return Ok(StatusPoll::NotFound),
        PodTarget::NotManaged => return Ok(StatusPoll::NotManaged),
        PodTarget::PodNotReady => return Ok(StatusPoll::PodNotReady),
    };
    let url = format!(
        "http://{pod_ip}:{control_port}{}",
        ControlCommand::Status.path()
    );
    match http.get(&url).timeout(STATUS_TIMEOUT).send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<StatusResponse>().await {
                Ok(status) => Ok(StatusPoll::Ok(status)),
                Err(err) => {
                    warn!(error = ?err, url, "failed to parse supervisor status reply");
                    Ok(StatusPoll::Unreachable)
                }
            }
        }
        Ok(response) => {
            warn!(status = %response.status(), url, "supervisor status route returned an error");
            Ok(StatusPoll::Unreachable)
        }
        Err(err) => {
            warn!(error = ?err, url, "failed to reach supervisor status route");
            Ok(StatusPoll::Unreachable)
        }
    }
}

#[cfg(test)]
#[path = "tests/supervisor.rs"]
mod tests;
