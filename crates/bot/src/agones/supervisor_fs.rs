//! The bot's client for the supervisor's filesystem, logs, and command routes —
//! the transport behind Gary's attach/inspect/edit and run-command tools. Each
//! call runs the same existence/managed/pod-IP gate as the lifecycle actions (via
//! [`resolve_managed_pod`]) and then issues one HTTP request to the in-pod
//! control API, mapping the reply to a [`FsOutcome`] the tool layer renders as
//! plain text.

use std::time::Duration;

use anyhow::Result;
use grizzly_control_api::{
    AnnounceRequest, CommandRequest, CommandResponse, ControlError, DirEntry, ListResponse,
    LogsQuery, LogsResponse, OCCUPANCY_PATH, OccupancyResponse, PathQuery, ReadResponse,
    RestoreRequest, RestoreResponse, WriteRequest, WriteResponse,
};
use kube::Client;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

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

/// The result of a targeted single-occurrence edit, before any restart. The
/// soft-failure variants leave the file untouched; only [`Self::Edited`] wrote.
pub(crate) enum EditOutcome {
    /// The edit applied and was written; carries the write result so the caller
    /// can mention the backup/restore path just like a plain write.
    Edited(WriteResponse),
    /// `old` wasn't found in the file, so there was nothing to replace.
    NoMatch,
    /// `old` appears more than once; carries the count so the caller can ask for
    /// a more specific anchor rather than guessing which one to change.
    Ambiguous(usize),
    /// `old` and `new` are identical — no change to make.
    Unchanged,
    /// The file was too large to read in full, so a safe read-modify-write round
    /// trip isn't possible; the caller should fall back to a full rewrite.
    TooLargeToEdit,
    /// The read or the write couldn't be served; carries the shared FS failure
    /// (payload dropped to unit — it's irrelevant to why the edit didn't land).
    Unserved(FsOutcome<()>),
}

/// The find-and-replace an edit applies, kept together so the two strings can't
/// be passed in the wrong order at a call site (and to keep
/// [`supervisor_edit_file`] within the argument budget).
pub(crate) struct Replacement<'a> {
    pub(crate) old: &'a str,
    pub(crate) new: &'a str,
}

/// Edit a file in place by replacing the single occurrence of `old` with `new`,
/// reading the current contents, applying the change, and writing it back (which
/// snapshots the prior version for `restore_file`). Refuses ambiguous or missing
/// anchors so a change can't silently hit the wrong text or clobber the file.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_edit_file(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
    edit: Replacement<'_>,
) -> Result<EditOutcome> {
    let Replacement { old, new } = edit;
    if old == new {
        return Ok(EditOutcome::Unchanged);
    }
    if old.is_empty() {
        // An empty anchor has no location to replace at; treat it as "not found"
        // rather than matching every character boundary.
        return Ok(EditOutcome::NoMatch);
    }

    let read =
        match supervisor_read_file(client, http, namespace, instance, control_port, path).await? {
            FsOutcome::Ok(read) => read,
            unserved @ (FsOutcome::NotFound
            | FsOutcome::NotManaged
            | FsOutcome::PodNotReady
            | FsOutcome::Unreachable
            | FsOutcome::Rejected(_)) => return Ok(into_unserved(unserved)),
        };
    if read.truncated {
        return Ok(EditOutcome::TooLargeToEdit);
    }

    let updated = match apply_unique_edit(&read.content, old, new) {
        EditApply::Replaced(text) => text,
        EditApply::NoMatch => return Ok(EditOutcome::NoMatch),
        EditApply::Ambiguous(count) => return Ok(EditOutcome::Ambiguous(count)),
    };

    match supervisor_write_file(
        client,
        http,
        namespace,
        instance,
        control_port,
        path,
        &updated,
    )
    .await?
    {
        FsOutcome::Ok(result) => Ok(EditOutcome::Edited(result)),
        unserved @ (FsOutcome::NotFound
        | FsOutcome::NotManaged
        | FsOutcome::PodNotReady
        | FsOutcome::Unreachable
        | FsOutcome::Rejected(_)) => Ok(into_unserved(unserved)),
    }
}

/// Re-tag a read/write [`FsOutcome`] that didn't succeed as the edit's
/// [`EditOutcome::Unserved`], dropping the (absent) payload.
fn into_unserved<T>(outcome: FsOutcome<T>) -> EditOutcome {
    EditOutcome::Unserved(map_payload(outcome, |_| ()))
}

/// The pure in-memory outcome of applying a single-occurrence replacement.
enum EditApply {
    Replaced(String),
    NoMatch,
    Ambiguous(usize),
}

/// Replace `old` with `new` only when it occurs exactly once. Occurrences are
/// counted non-overlapping, left to right — the same match semantics a reader
/// expects when they copy a unique snippet to anchor a change.
fn apply_unique_edit(content: &str, old: &str, new: &str) -> EditApply {
    match content.matches(old).count() {
        0 => EditApply::NoMatch,
        1 => EditApply::Replaced(content.replacen(old, new, 1)),
        count => EditApply::Ambiguous(count),
    }
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

/// Read the instance's live connected-player count over the control API.
/// `Ok(FsOutcome::Ok(None))` means the count is *unknown* — the game has no RCON,
/// or its console couldn't be reached — which the caller must treat as "can't
/// tell", never as an empty server. Used to check occupancy before a disruptive
/// action so a restart never silently kicks a live session.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_occupancy(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
) -> Result<FsOutcome<Option<u32>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{OCCUPANCY_PATH}");
    let response = http.get(&url).timeout(FS_TIMEOUT).send().await;
    let outcome: FsOutcome<OccupancyResponse> = finish(response, &url).await;
    Ok(map_payload(outcome, |occupancy| occupancy.players))
}

/// Broadcast a message to everyone on the instance's server, best-effort. This is
/// the in-game audit trail for Gary's mutating actions, so a failure to deliver it
/// must never block or fail the action that triggered it — every unhappy path is
/// logged and swallowed rather than propagated. A paused or RCON-less server
/// simply gets no broadcast.
pub(crate) async fn supervisor_announce(
    client: &Client,
    http: &reqwest::Client,
    namespace: &str,
    instance: &str,
    control_port: u16,
    message: &str,
) {
    let pod_ip = match resolve_managed_pod(client, namespace, instance).await {
        Ok(PodTarget::Ready(pod_ip)) => pod_ip,
        Ok(_) => {
            debug!(instance, "skipping in-game announce; pod not ready");
            return;
        }
        Err(err) => {
            warn!(error = ?err, instance, "failed to locate pod for in-game announce");
            return;
        }
    };
    let url = format!("http://{pod_ip}:{control_port}/announce");
    match http
        .post(&url)
        .json(&AnnounceRequest {
            message: message.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await
    {
        Ok(reply) if reply.status().is_success() => {}
        Ok(reply) => debug!(status = %reply.status(), instance, "in-game announce not delivered"),
        Err(err) => warn!(error = ?err, url, "failed to reach the announce route"),
    }
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

#[cfg(test)]
#[path = "tests/supervisor_fs.rs"]
mod tests;
