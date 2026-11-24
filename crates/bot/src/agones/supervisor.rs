use std::time::Duration;

use anyhow::{Context, Result};
use grizzly_control_api::{
    ControlCommand, ControlError, ControlOk, ProcessPhase, ResultKind, StatusResponse,
};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::ListParams;
use kube::{Api, Client};
use tracing::{debug, warn};

use super::control_client::ControlClient;
use super::labels::{GAMESERVER_SELECTOR_KEY, is_managed};
use super::provision::{DeletionState, deletion_state};
use super::types::GameServer;

pub(crate) const CONTROL_MUTATION_TIMEOUT: Duration = Duration::from_mins(3);

const STATUS_TIMEOUT: Duration = Duration::from_secs(10);

const READY_WAIT_TIMEOUT: Duration = Duration::from_mins(5);
const READY_POLL_INTERVAL: Duration = Duration::from_secs(3);

pub(crate) enum RuntimeState {
    PodUp,
    Terminating,
    Down,
    Absent,
}

pub(crate) enum SupervisorOutcome {
    Paused,
    Resumed,
    Restarted,
    AlreadyStopped,
    AlreadyRunning,
    PodNotReady,
    Unreachable,
    Failed(String),
    NotFound,
    NotManaged,
}

pub(crate) async fn supervisor_stop(
    client: &Client,
    http: &ControlClient,
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

pub(crate) async fn supervisor_start(
    client: &Client,
    http: &ControlClient,
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

pub(crate) async fn supervisor_restart(
    client: &Client,
    http: &ControlClient,
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

pub(crate) async fn instance_runtime_state(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<RuntimeState> {
    let gameservers: Api<GameServer> = Api::namespaced(client.clone(), namespace);
    if let Some(gameserver) = gameservers
        .get_opt(instance)
        .await
        .with_context(|| format!("failed to read gameserver {instance}"))?
    {
        return Ok(match deletion_state(&gameserver.metadata) {
            DeletionState::Live => RuntimeState::PodUp,
            DeletionState::Terminating => RuntimeState::Terminating,
        });
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

pub(crate) enum PodTarget {
    Ready(String),
    NotFound,
    NotManaged,
    PodNotReady,
}

pub(crate) async fn resolve_managed_pod(
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
    http: &ControlClient,
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

fn running_pod_ip(pod: &Pod) -> Option<String> {
    if deletion_state(&pod.metadata) == DeletionState::Terminating {
        return None;
    }
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

pub(crate) enum ReadyWait {
    Ready,
    Crashed,
    Stopped,
    TimedOut,
    NotFound,
    NotManaged,
}

pub(crate) enum StatusPoll {
    Ok(StatusResponse),
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable,
}

pub(crate) async fn wait_for_ready(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<ReadyWait> {
    wait_for_ready_within(
        client,
        http,
        namespace,
        instance,
        control_port,
        READY_WAIT_TIMEOUT,
    )
    .await
}

pub(crate) async fn wait_for_ready_within(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    timeout: Duration,
) -> Result<ReadyWait> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut ticker = tokio::time::interval(READY_POLL_INTERVAL);

    loop {
        ticker.tick().await;
        match supervisor_status(client, http, namespace, instance, control_port).await? {
            StatusPoll::Ok(status) => match status.process {
                ProcessPhase::Running => return Ok(ReadyWait::Ready),
                ProcessPhase::Crashed => return Ok(ReadyWait::Crashed),
                ProcessPhase::Stopped => return Ok(ReadyWait::Stopped),
                ProcessPhase::Starting | ProcessPhase::Stopping => {}
            },
            StatusPoll::NotFound => return Ok(ReadyWait::NotFound),
            StatusPoll::NotManaged => return Ok(ReadyWait::NotManaged),
            StatusPoll::PodNotReady | StatusPoll::Unreachable => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(ReadyWait::TimedOut);
        }
    }
}

pub(crate) enum ControlReady {
    Reachable,
    NotFound,
    NotManaged,
    TimedOut,
}

pub(crate) async fn wait_for_control_reachable(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<ControlReady> {
    let deadline = tokio::time::Instant::now() + READY_WAIT_TIMEOUT;
    let mut ticker = tokio::time::interval(READY_POLL_INTERVAL);

    loop {
        ticker.tick().await;
        match supervisor_status(client, http, namespace, instance, control_port).await? {
            StatusPoll::Ok(_) => return Ok(ControlReady::Reachable),
            StatusPoll::NotFound => return Ok(ControlReady::NotFound),
            StatusPoll::NotManaged => return Ok(ControlReady::NotManaged),
            StatusPoll::PodNotReady | StatusPoll::Unreachable => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(ControlReady::TimedOut);
        }
    }
}

pub(crate) async fn supervisor_status(
    client: &Client,
    http: &ControlClient,
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
            debug!(status = %response.status(), url, "supervisor status route returned an error");
            Ok(StatusPoll::Unreachable)
        }
        Err(err) => {
            debug!(error = ?err, url, "failed to reach supervisor status route");
            Ok(StatusPoll::Unreachable)
        }
    }
}

#[cfg(test)]
#[path = "tests/supervisor.rs"]
mod tests;
