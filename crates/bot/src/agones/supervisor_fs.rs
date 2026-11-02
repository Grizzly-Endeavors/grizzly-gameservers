//! The bot's client for the supervisor's filesystem, logs, and command routes —
//! the transport behind Gary's attach/inspect/edit and run-command tools. Each
//! call runs the same existence/managed/pod-IP gate as the lifecycle actions (via
//! [`Servers::locate`]) and then issues one HTTP request to the in-pod
//! control API, mapping the reply to a [`FsOutcome`] the tool layer renders as
//! plain text.

use std::time::Duration;

use anyhow::Result;
use grizzly_control_api::{
    ANNOUNCE_PATH, AnnounceRequest, COMMAND_PATH, CommandRequest, CommandResponse, ControlError,
    DirEntry, EditRequest, EditResponse, FS_EDIT_PATH, FS_LIST_PATH, FS_READ_PATH, FS_RESTORE_PATH,
    FS_WRITE_PATH, LOGS_PATH, ListResponse, LogsQuery, LogsResponse, OCCUPANCY_PATH,
    OccupancyResponse, PathQuery, ReadQuery, ReadResponse, RestoreRequest, RestoreResponse,
    SHELL_PATH, ShellRequest, ShellResponse, WriteRequest, WriteResponse,
};
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use super::control_client::ControlClient;
use super::directory::Servers;
use super::supervisor::PodTarget;

/// Per-request timeout for the filesystem/logs calls. These touch only the PVC
/// or an in-memory buffer, so they're fast; this is a guard against a wedged pod,
/// not a real work budget.
const FS_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-request timeout for the shell route. Deliberately longer than the
/// supervisor's own 30s ceiling on a command: the supervisor kills a slow command
/// and answers with `timed_out`, and that answer is more useful to the agent than
/// a transport error, so the client has to still be waiting when it arrives.
const SHELL_TIMEOUT: Duration = Duration::from_secs(45);

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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    path: &str,
) -> Result<FsOutcome<Vec<DirEntry>>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{FS_LIST_PATH}");
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

/// Which part of a file to ask for.
///
/// The two exist because the two kinds of caller want different things. A
/// program reading a file it will parse wants all of it and doesn't care where
/// the lines fall; the model wants a bounded window it can address and continue
/// from. Spelling it as a choice keeps a caller from inheriting the other's
/// default by omission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadWindow {
    /// The whole file, up to the supervisor's read cap. What a parser wants.
    Whole,
    /// `count` lines starting at 1-based `start`. What the model reads.
    Lines { start: usize, count: usize },
}

/// Read a UTF-8 file under the instance's data root.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_read_file(
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    path: &str,
    window: ReadWindow,
) -> Result<FsOutcome<ReadResponse>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{FS_READ_PATH}");
    let query = match window {
        ReadWindow::Whole => ReadQuery {
            path: path.to_owned(),
            start_line: None,
            line_count: None,
        },
        ReadWindow::Lines { start, count } => ReadQuery {
            path: path.to_owned(),
            start_line: Some(start),
            line_count: Some(count),
        },
    };
    let response = http
        .get(&url)
        .query(&query)
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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    path: &str,
    content: &str,
) -> Result<FsOutcome<WriteResponse>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{FS_WRITE_PATH}");
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
/// snapshotting the prior version for `restore_file`. Refuses ambiguous or
/// missing anchors so a change can't silently hit the wrong text or clobber the
/// file.
///
/// The replacement runs in the pod, so the file never crosses the wire and what
/// can be edited doesn't depend on what fits in a read.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_edit_file(
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
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

    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(into_unserved::<()>(not_ready.into_outcome())),
    };
    let url = format!("{base}{FS_EDIT_PATH}");
    let response = http
        .post(&url)
        .json(&EditRequest {
            path: path.to_owned(),
            old: old.to_owned(),
            new: new.to_owned(),
        })
        .timeout(FS_TIMEOUT)
        .send()
        .await;

    read_edit_reply(response, &url).await
}

/// Turn the edit route's reply into an [`EditOutcome`].
///
/// Status is inspected directly rather than through [`finish`], which folds every
/// non-success into `Rejected` — fine for the refusals, but the success body
/// carries a match count that decides between three different outcomes.
async fn read_edit_reply(
    response: reqwest::Result<reqwest::Response>,
    url: &str,
) -> Result<EditOutcome> {
    let reply = match response {
        Ok(reply) => reply,
        Err(err) => {
            warn!(error = ?err, url, "failed to reach supervisor edit route");
            return Ok(EditOutcome::Unserved(FsOutcome::Unreachable));
        }
    };
    let status = reply.status();
    if !status.is_success() {
        return Ok(match reply.json::<ControlError>().await {
            Ok(error) => EditOutcome::Unserved(FsOutcome::Rejected(error.error)),
            Err(err) => {
                warn!(%status, error = ?err, url, "supervisor edit route returned an unreadable error");
                EditOutcome::Unserved(FsOutcome::Unreachable)
            }
        });
    }
    Ok(match reply.json::<EditResponse>().await {
        Ok(edit) => match edit.occurrences {
            1 => EditOutcome::Edited(WriteResponse {
                path: edit.path,
                backed_up: edit.backed_up,
            }),
            0 => EditOutcome::NoMatch,
            count => EditOutcome::Ambiguous(count),
        },
        Err(err) => {
            warn!(error = ?err, url, "failed to parse supervisor edit reply");
            EditOutcome::Unserved(FsOutcome::Unreachable)
        }
    })
}

/// Re-tag a read/write [`FsOutcome`] that didn't succeed as the edit's
/// [`EditOutcome::Unserved`], dropping the (absent) payload.
fn into_unserved<T>(outcome: FsOutcome<T>) -> EditOutcome {
    EditOutcome::Unserved(map_payload(outcome, |_| ()))
}

/// Restore a file under the instance's data root from its last snapshot.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_restore_file(
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    path: &str,
) -> Result<FsOutcome<RestoreResponse>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{FS_RESTORE_PATH}");
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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    lines: Option<usize>,
) -> Result<FsOutcome<Vec<String>>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{LOGS_PATH}");
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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    command: &str,
) -> Result<FsOutcome<CommandResponse>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{COMMAND_PATH}");
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

/// Run one read-only shell command inside the instance's game container.
///
/// `Ok(FsOutcome::Ok(None))` means this server's supervisor predates the shell
/// route — a live server keeps the binary baked into its running image until it
/// next starts, so that is an ordinary state rather than an error. It is
/// distinguished here because the older supervisor answers an unknown path from
/// its control-command fallback, whose 404 body is indistinguishable from a
/// genuine rejection once it has been read.
///
/// The command is sandboxed in-pod (see the supervisor's `shell` module), so this
/// carries no authority the caller has to police: a command that tries to write
/// the game's files is refused by the kernel. A non-zero exit arrives as an
/// ordinary [`ShellResponse`], not an error — a failed `grep` is a result.
///
/// # Errors
///
/// Returns an error if the cluster can't be queried to locate the pod.
pub(crate) async fn supervisor_run_shell(
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    command: &str,
) -> Result<FsOutcome<Option<ShellResponse>>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{SHELL_PATH}");
    let response = http
        .post(&url)
        .json(&ShellRequest {
            command: command.to_owned(),
        })
        .timeout(SHELL_TIMEOUT)
        .send()
        .await;
    if let Ok(reply) = &response
        && reply.status() == reqwest::StatusCode::NOT_FOUND
    {
        debug!(
            url,
            "supervisor has no shell route; it predates the sandboxed shell"
        );
        return Ok(FsOutcome::Ok(None));
    }
    Ok(map_payload(finish(response, &url).await, Some))
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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
) -> Result<FsOutcome<Option<u32>>> {
    let base = match target_pod(servers, instance).await? {
        Ok(base) => base,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("{base}{OCCUPANCY_PATH}");
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
    servers: Servers<'_>,
    http: &ControlClient,
    instance: &str,
    message: &str,
) {
    let base = match servers.locate(instance).await {
        Ok(PodTarget::Ready(base)) => base,
        Ok(_) => {
            debug!(instance, "skipping in-game announce; pod not ready");
            return;
        }
        Err(err) => {
            warn!(error = ?err, instance, "failed to locate pod for in-game announce");
            return;
        }
    };
    let url = format!("{base}{ANNOUNCE_PATH}");
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

/// Resolve the instance's control-API base URL, or the reason it isn't reachable.
async fn target_pod(
    servers: Servers<'_>,
    instance: &str,
) -> Result<std::result::Result<String, Unavailable>> {
    let target = servers.locate(instance).await?;
    Ok(match target {
        PodTarget::Ready(base) => Ok(base),
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
