use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep};

const PROBE_INTERVAL: Duration = Duration::from_secs(2);

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

#[derive(Clone)]
pub(crate) struct LogReadyWatch {
    pattern: Arc<str>,
    ready_tx: mpsc::Sender<()>,
}

impl LogReadyWatch {
    pub(crate) fn new(pattern: Arc<str>, ready_tx: mpsc::Sender<()>) -> Self {
        Self { pattern, ready_tx }
    }

    pub(crate) fn matches(&self, line: &str) -> bool {
        line.contains(&*self.pattern)
    }

    pub(crate) fn try_signal(&self) -> Result<(), mpsc::error::TrySendError<()>> {
        self.ready_tx.try_send(())
    }
}

#[cfg(test)]
#[path = "tests/readiness.rs"]
mod tests;
