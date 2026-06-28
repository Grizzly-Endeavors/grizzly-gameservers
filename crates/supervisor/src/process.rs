use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tracing::warn;

use crate::config::SupervisorConfig;
use crate::logs::LogBuffer;

/// Launch the supervised game-server process. Inherits the environment so the
/// itzg image reads its `EULA`/`MEMORY`/etc. knobs, and `kill_on_drop` so a
/// supervisor crash can't orphan the game. stdout/stderr are piped (not
/// inherited) so [`capture_output`] can both tee them to the supervisor's own
/// output and retain a tail for the control API.
///
/// # Errors
///
/// Returns an error if the child command cannot be spawned.
pub fn spawn(cfg: &SupervisorConfig) -> Result<Child> {
    let mut cmd = Command::new(&cfg.child_command);
    cmd.kill_on_drop(true);
    // No interactive console; stop/restart go through SIGTERM, not stdin.
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.spawn()
        .with_context(|| format!("failed to spawn child command {:?}", cfg.child_command))
}

/// Drain the child's piped stdout and stderr into `logs`, re-emitting each line
/// to the supervisor's own stdout/stderr so `kubectl logs` still shows the game's
/// output. The reader tasks end on their own when the pipes close at child exit.
pub fn capture_output(child: &mut Child, logs: &Arc<LogBuffer>) {
    if let Some(stdout) = child.stdout.take() {
        spawn_line_pump(stdout, Arc::clone(logs), Stream::Stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_line_pump(stderr, Arc::clone(logs), Stream::Stderr);
    }
}

/// Which standard stream a captured line came from, so it can be re-emitted on
/// the matching supervisor stream.
#[derive(Clone, Copy)]
enum Stream {
    Stdout,
    Stderr,
}

fn spawn_line_pump<R>(reader: R, logs: Arc<LogBuffer>, stream: Stream)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    match stream {
                        Stream::Stdout => println!("{line}"),
                        Stream::Stderr => eprintln!("{line}"),
                    }
                    logs.push(line);
                }
                Ok(None) => break,
                Err(err) => {
                    warn!(error = %err, "failed reading child output stream");
                    break;
                }
            }
        }
    });
}

/// Gracefully stop the child: SIGTERM (which itzg's `mc-server-runner` traps into
/// a world-save), wait up to `grace`, then SIGKILL as a last resort. Reaps the
/// child either way.
///
/// # Errors
///
/// Returns an error if waiting on or killing the child fails at the OS level.
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

/// Send SIGTERM to `pid`, logging (not failing) if the signal can't be delivered
/// — a delivery failure usually means the child already exited.
///
/// # Errors
///
/// Returns an error only if the pid does not fit the platform `pid_t`.
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

/// Await a child's exit when one is running, or never resolve when the slot is
/// empty (stopped). Lets the runner's `select!` keep a single child-exit arm
/// without spinning once the process is intentionally down.
///
/// # Errors
///
/// Returns the error from the underlying `wait` if the OS can't reap the child.
pub async fn wait_optional(child: &mut Option<Child>) -> std::io::Result<ExitStatus> {
    match child {
        Some(running) => running.wait().await,
        None => std::future::pending().await,
    }
}
