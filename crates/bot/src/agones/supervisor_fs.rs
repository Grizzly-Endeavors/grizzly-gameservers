use std::time::Duration;

use anyhow::Result;
use grizzly_control_api::{
    ANNOUNCE_PATH, AnnounceRequest, COMMAND_PATH, CommandRequest, CommandResponse, ControlError,
    DirEntry, EditRequest, EditResponse, FS_EDIT_PATH, FS_LIST_PATH, FS_READ_PATH, FS_RESTORE_PATH,
    FS_WRITE_PATH, LOGS_PATH, ListResponse, LogsQuery, LogsResponse, OCCUPANCY_PATH,
    OccupancyResponse, PathQuery, ReadQuery, ReadResponse, RestoreRequest, RestoreResponse,
    SHELL_PATH, ShellRequest, ShellResponse, WriteRequest, WriteResponse,
};
use kube::Client;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use super::control_client::ControlClient;
use super::supervisor::{PodTarget, resolve_managed_pod};

const FS_TIMEOUT: Duration = Duration::from_secs(30);

const SHELL_TIMEOUT: Duration = Duration::from_secs(45);

pub(crate) enum FsOutcome<T> {
    Ok(T),
    NotFound,
    NotManaged,
    PodNotReady,
    Unreachable,
    Rejected(String),
}

pub(crate) async fn supervisor_list_files(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
) -> Result<FsOutcome<Vec<DirEntry>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{FS_LIST_PATH}");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadWindow {
    Whole,
    Lines { start: usize, count: usize },
}

pub(crate) async fn supervisor_read_file(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
    window: ReadWindow,
) -> Result<FsOutcome<ReadResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{FS_READ_PATH}");
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

pub(crate) async fn supervisor_write_file(
    client: &Client,
    http: &ControlClient,
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
    let url = format!("http://{pod_ip}:{control_port}{FS_WRITE_PATH}");
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

pub(crate) enum EditOutcome {
    Edited(WriteResponse),
    NoMatch,
    Ambiguous(usize),
    Unchanged,
    Unserved(FsOutcome<()>),
}

pub(crate) struct Replacement<'a> {
    pub(crate) old: &'a str,
    pub(crate) new: &'a str,
}

pub(crate) async fn supervisor_edit_file(
    client: &Client,
    http: &ControlClient,
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
        return Ok(EditOutcome::NoMatch);
    }

    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(into_unserved::<()>(not_ready.into_outcome())),
    };
    let url = format!("http://{pod_ip}:{control_port}{FS_EDIT_PATH}");
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

fn into_unserved<T>(outcome: FsOutcome<T>) -> EditOutcome {
    EditOutcome::Unserved(map_payload(outcome, |_| ()))
}

pub(crate) async fn supervisor_restore_file(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    path: &str,
) -> Result<FsOutcome<RestoreResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{FS_RESTORE_PATH}");
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

pub(crate) async fn supervisor_read_logs(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    lines: Option<usize>,
) -> Result<FsOutcome<Vec<String>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{LOGS_PATH}");
    let response = http
        .get(&url)
        .query(&LogsQuery { lines })
        .timeout(FS_TIMEOUT)
        .send()
        .await;
    let outcome: FsOutcome<LogsResponse> = finish(response, &url).await;
    Ok(map_payload(outcome, |logs| logs.lines))
}

pub(crate) async fn supervisor_send_command(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    command: &str,
) -> Result<FsOutcome<CommandResponse>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{COMMAND_PATH}");
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

pub(crate) async fn supervisor_run_shell(
    client: &Client,
    http: &ControlClient,
    namespace: &str,
    instance: &str,
    control_port: u16,
    command: &str,
) -> Result<FsOutcome<Option<ShellResponse>>> {
    let pod_ip = match target_pod(client, namespace, instance).await? {
        Ok(pod_ip) => pod_ip,
        Err(not_ready) => return Ok(not_ready.into_outcome()),
    };
    let url = format!("http://{pod_ip}:{control_port}{SHELL_PATH}");
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

pub(crate) async fn supervisor_occupancy(
    client: &Client,
    http: &ControlClient,
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

pub(crate) async fn supervisor_announce(
    client: &Client,
    http: &ControlClient,
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
    let url = format!("http://{pod_ip}:{control_port}{ANNOUNCE_PATH}");
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
