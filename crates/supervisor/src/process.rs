use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::warn;

use crate::config::SupervisorConfig;
use crate::launch_log::LaunchLog;
use crate::logs::LogBuffer;
use crate::rcon::RconRuntime;
use crate::readiness::LogReadyWatch;

pub fn spawn(
    cfg: &SupervisorConfig,
    rcon: Option<&RconRuntime>,
    overlay: &[(String, String)],
) -> Result<Child> {
    let mut cmd = Command::new(&cfg.child_command);
    cmd.kill_on_drop(true);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    for (key, value) in overlay {
        cmd.env(key, value);
    }
    if let Some(rcon) = rcon {
        cmd.env(&cfg.rcon_password_env, rcon.password());
    }
    cmd.spawn()
        .with_context(|| format!("failed to spawn child command {:?}", cfg.child_command))
}

pub(crate) fn capture_output(
    child: &mut Child,
    logs: &Arc<LogBuffer>,
    launch: &Arc<LaunchLog>,
    chat_tx: Option<&mpsc::Sender<String>>,
    ready: Option<&LogReadyWatch>,
) {
    if let Some(stdout) = child.stdout.take() {
        launch.pump_started();
        spawn_line_pump(
            stdout,
            Arc::clone(logs),
            Arc::clone(launch),
            Stream::Stdout,
            chat_tx.cloned(),
            ready.cloned(),
        );
    }
    if let Some(stderr) = child.stderr.take() {
        launch.pump_started();
        spawn_line_pump(
            stderr,
            Arc::clone(logs),
            Arc::clone(launch),
            Stream::Stderr,
            chat_tx.cloned(),
            ready.cloned(),
        );
    }
}

#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

fn spawn_line_pump<R>(
    reader: R,
    logs: Arc<LogBuffer>,
    launch: Arc<LaunchLog>,
    stream: Stream,
    chat_tx: Option<mpsc::Sender<String>>,
    ready: Option<LogReadyWatch>,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut ready_signalled = false;
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    match stream {
                        Stream::Stdout => println!("{line}"),
                        Stream::Stderr => eprintln!("{line}"),
                    }
                    if let Some(watch) = ready.as_ref()
                        && !ready_signalled
                        && watch.matches(&line)
                    {
                        ready_signalled = true;
                        if watch.try_signal().is_err() {}
                    }
                    if let Some(tx) = chat_tx.as_ref()
                        && tx.try_send(line.clone()).is_err()
                    {}
                    launch.push(line.clone());
                    logs.push(line);
                }
                Ok(None) => break,
                Err(err) => {
                    warn!(error = %err, "failed reading child output stream");
                    break;
                }
            }
        }
        launch.pump_finished();
    });
}

pub async fn graceful_stop(child: &mut Child, grace: Duration) -> Result<()> {
    if let Some(pid) = child.id() {
        request_terminate(pid)?;
    }
    match timeout(grace, child.wait()).await {
        Ok(Ok(_status)) => Ok(()),
        Ok(Err(err)) => Err(err).context("failed waiting for child to exit after SIGTERM"),
        Err(_elapsed) => {
            warn!(
                grace_secs = grace.as_secs(),
                "child did not exit within grace window; sending SIGKILL"
            );
            child.start_kill().context("failed to SIGKILL child")?;
            child
                .wait()
                .await
                .context("failed to reap child after SIGKILL")?;
            Ok(())
        }
    }
}

fn request_terminate(pid: u32) -> Result<()> {
    let pid = i32::try_from(pid).context("child pid does not fit in pid_t")?;
    let rc = raw_kill(pid, libc::SIGTERM);
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        warn!(error = %err, pid, "failed to deliver SIGTERM; child may have already exited");
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "FFI boundary: libc::kill signals a known pid, touches no memory"
)]
fn raw_kill(pid: i32, signal: i32) -> i32 {
    unsafe { libc::kill(pid, signal) }
}

pub async fn wait_optional(child: &mut Option<Child>) -> std::io::Result<ExitStatus> {
    match child {
        Some(running) => running.wait().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "tests/process.rs"]
mod tests;
