//! The bot's client for the supervisor's filesystem, logs, and command routes —
//! the transport behind Gary's attach/inspect/edit and run-command tools. Each
//! call runs the same existence/managed/pod-IP gate as the lifecycle actions (via
//! [`resolve_managed_pod`]) and then issues one HTTP request to the in-pod
//! control API, mapping the reply to a [`FsOutcome`] the tool layer renders as
//! plain text.

use std::time::Duration;

use anyhow::Result;
use grizzly_control_api::{
    CommandRequest, CommandResponse, ControlError, DirEntry, ListResponse, LogsQuery, LogsResponse,
    PathQuery, ReadResponse, RestoreRequest, RestoreResponse, WriteRequest, WriteResponse,
};
use kube::Client;
use serde::de::DeserializeOwned;
use tracing::warn;

use super::supervisor::{PodTarget, resolve_managed_pod};

/// Per-request timeout for the filesystem/logs calls. These touch only the PVC
/// or an in-memory buffer, so they're fast; this is a guard against a wedged pod,
/// not a real work budget.
const FS_TIMEOUT: Duration = Duration::from_secs(30);

/// The result of a filesystem/logs call against an instance's supervisor.
/// Distinguishes the payload from the ways the request can't be served, so the
/// tool layer can phrase an accurate, plain-language reply.
pub(crate) enum FsOutcome<T> {
    /// The operation succeeded; carries the typed response payload.
    Ok(T),
    /// No live `GameServer` by that name.
    NotFound,
    /// The instance exists but isn't shim-managed.
    NotManaged,
    /// The pod exists but isn't far enough along to have a reachable control API.
    PodNotReady,
    /// The control API couldn't be reached or returned an unparseable response.
    Unreachable,
    /// The supervisor refused the request (bad path, not text, too large, no
    /// snapshot, …); carries its developer-facing message for Gary to relay.
    Rejected(String),
}

/// List a directory under the instance's data root.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_list_files(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
) -> Result<FsOutcome<Vec<DirEntry>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/fs/list");
    let response = http
        .get(&url)
        .query(&PathQuery {
            path: path.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    let outcome: FsOutcome<ListResponse> = finish(response, &url).await;
    Ok(map_payload(outcome, |list| list.entries))
}

/// Read a UTF-8 file under the instance's data root.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_read_file(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
) -> Result<FsOutcome<ReadResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/fs/read");
    let response = http
        .get(&url)
        .query(&PathQuery {
            path: path.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    Ok(finish(response, &url).await)
}

/// Overwrite a file under the instance's data root, snapshotting it first.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_write_file(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
    content: &str,
) -> Result<FsOutcome<WriteResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/fs/write");
    let response = http
        .post(&url)
        .json(&WriteRequest {
            path: path.to_owned(),
            content: content.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    Ok(finish(response, &url).await)
}

/// Restore a file under the instance's data root from its last snapshot.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_restore_file(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
) -> Result<FsOutcome<RestoreResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/fs/restore");
    let response = http
        .post(&url)
        .json(&RestoreRequest {
            path: path.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    Ok(finish(response, &url).await)
}

/// Tail the most recent lines of the instance's captured output.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_read_logs(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    lines: Option<usize>,
) -> Result<FsOutcome<Vec<String>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/logs");
    let response = http
        .get(&url)
        .query(&LogsQuery { lines })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    let outcome: FsOutcome<LogsResponse> = finish(response, &url).await;
    Ok(map_payload(outcome, |logs| logs.lines))
}

/// Run one in-game console command against the instance's server over RCON. A
/// [`FsOutcome::Rejected`] here carries the supervisor's reason (e.g. RCON not
/// enabled for the game, or the console couldn't be reached).
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_send_command(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    command: &str,
) -> Result<FsOutcome<CommandResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}/command");
    let response = http
        .post(&url)
        .json(&CommandRequest {
            command: command.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    Ok(finish(response, &url).await)
}

/// The non-ready resolutions, separated from the pod IP so a caller can turn one
/// into an [`FsOutcome`] of whatever payload type its operation returns.
enum Unavailable {
    NotFound,
    NotManaged,
    PodNotReady,
}

impl Unavailable {
    fn into_outcome<T>(self) -> FsOutcome<T> {
        match self {
            Self::NotFound => FsOutcome::NotFound,
            Self::NotManaged => FsOutcome::NotManaged,
            Self::PodNotReady => FsOutcome::PodNotReady,
        }
    }
}

/// Resolve the instance's pod IP, or the reason it isn't reachable.
async fn target_pod(
    client: &Client,
    namespace: &str,
    instance: &str,
) -> Result<std::result::Result<String, Unavailable>> {
    let target = resolve_managed_pod(client, namespace, instance).await?;
    Ok(match target {
        PodTarget::Ready(pod_ip) => Ok(pod_ip),
        PodTarget::NotFound => Err(Unavailable::NotFound),
        PodTarget::NotManaged => Err(Unavailable::NotManaged),
        PodTarget::PodNotReady => Err(Unavailable::PodNotReady),
    })
}

/// Map an HTTP reply to an [`FsOutcome`]: a 2xx parses into `T`, a 4xx/5xx
/// surfaces the supervisor's [`ControlError`] message as `Rejected`, and a
/// transport failure or unparseable body is `Unreachable`.
async fn finish<T: DeserializeOwned>(
    response: reqwest::Result<reqwest::Response>,
    url: &str,
) -> FsOutcome<T> {
    match response {
        Ok(reply) => {
            let status = reply.status();
            if status.is_success() {
                match reply.json::<T>().await {
                    Ok(payload) => FsOutcome::Ok(payload),
                    Err(err) => {
                        warn!(error = ?err, url, "failed to parse supervisor fs reply");
                        FsOutcome::Unreachable
                    }
                }
            } else {
                match reply.json::<ControlError>().await {
                    Ok(error) => FsOutcome::Rejected(error.error),
                    Err(err) => {
                        warn!(%status, error = ?err, url, "supervisor fs route returned an unreadable error");
                        FsOutcome::Unreachable
                    }
                }
            }
        }
        Err(err) => {
            warn!(error = ?err, url, "failed to reach supervisor fs route");
            FsOutcome::Unreachable
        }
    }
}

/// Transform the success payload of an [`FsOutcome`] while preserving every
/// non-success variant unchanged.
fn map_payload<T, U>(outcome: FsOutcome<T>, transform: impl FnOnce(T) -> U) -> FsOutcome<U> {
    match outcome {
        FsOutcome::Ok(payload) => FsOutcome::Ok(transform(payload)),
        FsOutcome::NotFound => FsOutcome::NotFound,
        FsOutcome::NotManaged => FsOutcome::NotManaged,
        FsOutcome::PodNotReady => FsOutcome::PodNotReady,
        FsOutcome::Unreachable => FsOutcome::Unreachable,
        FsOutcome::Rejected(message) => FsOutcome::Rejected(message),
    }
}
