use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio_util::sync::CancellationToken;

#[derive(Debug)]
struct Registration {
    id: u64,
    token: CancellationToken,
}

#[derive(Debug, Default)]
pub(crate) struct TurnInterrupts {
    running: Mutex<HashMap<u64, Registration>>,
    next_id: AtomicU64,
}

impl TurnInterrupts {
    pub(crate) fn register(&self, channel: u64) -> (CancellationToken, InterruptGuard<'_>) {
        let token = CancellationToken::new();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut running) = self.running.lock() {
            running.insert(
                channel,
                Registration {
                    id,
                    token: token.clone(),
                },
            );
        }
        (
            token,
            InterruptGuard {
                interrupts: self,
                channel,
                id,
            },
        )
    }

    pub(crate) fn cancel(&self, channel: u64) -> bool {
        let Ok(running) = self.running.lock() else {
            return false;
        };
        let Some(registration) = running.get(&channel) else {
            return false;
        };
        registration.token.cancel();
        true
    }

    #[cfg(test)]
    pub(crate) fn is_running(&self, channel: u64) -> bool {
        self.running
            .lock()
            .is_ok_and(|running| running.contains_key(&channel))
    }
}

#[derive(Debug)]
pub(crate) struct InterruptGuard<'a> {
    interrupts: &'a TurnInterrupts,
    channel: u64,
    id: u64,
}

impl Drop for InterruptGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut running) = self.interrupts.running.lock()
            && running
                .get(&self.channel)
                .is_some_and(|registration| registration.id == self.id)
        {
            running.remove(&self.channel);
        }
    }
}

#[cfg(test)]
#[path = "tests/interrupt.rs"]
mod tests;
