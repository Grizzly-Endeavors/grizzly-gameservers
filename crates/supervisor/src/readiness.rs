use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

/// Poll interval between connection attempts while waiting for the game to bind.
const PROBE_INTERVAL: Duration = Duration::from_secs(2);

/// Wait until the game process is accepting TCP connections on `port`, giving up
/// after `give_up`. Used to gate the one-shot Agones `/ready` call: the server is
/// only "ready" once a client could actually connect.
///
/// Returns `true` once a connection succeeds, `false` if `give_up` elapses first
/// (e.g. first-boot world generation overran the window).
pub async fn wait_accepting(port: u16, give_up: Duration) -> bool {
    let deadline = Instant::now() + give_up;
    loop {
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .is_ok()
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(PROBE_INTERVAL).await;
    }
}

/// Log-line readiness for games that never open a TCP port the connect probe
/// could use — UDP-only servers (Valheim) that would otherwise never signal
/// Agones `Ready`. The supervisor already tees every captured output line, so a
/// game declares a "server ready" marker (`SUPERVISOR_READY_LOG_PATTERN`) and the
/// line pump signals readiness the first time a line contains it. Cheap
/// substring match, not a regex: the marker is an operator-chosen fixed string.
#[derive(Clone)]
pub(crate) struct LogReadyWatch {
    pattern: Arc<str>,
    ready_tx: mpsc::Sender<()>,
}

impl LogReadyWatch {
    pub(crate) fn new(pattern: Arc<str>, ready_tx: mpsc::Sender<()>) -> Self {
        Self { pattern, ready_tx }
    }

    /// Whether `line` contains the readiness marker.
    pub(crate) fn matches(&self, line: &str) -> bool {
        line.contains(&*self.pattern)
    }

    /// Signal readiness to the runner. Non-blocking and best-effort: a full
    /// (already-signalled) or dropped channel is fine — Agones `/ready` is
    /// idempotent and latched after the first success, so the caller stops
    /// checking once this is invoked.
    ///
    /// # Errors
    ///
    /// Returns the [`mpsc::error::TrySendError`] when the channel is full or the
    /// runner has gone away; both are benign and expected.
    pub(crate) fn try_signal(&self) -> Result<(), mpsc::error::TrySendError<()>> {
        self.ready_tx.try_send(())
    }
}

#[cfg(test)]
#[path = "tests/readiness.rs"]
mod tests;
