use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug)]
pub struct Heartbeat {
    beating: Arc<AtomicBool>,
}

impl Heartbeat {
    #[must_use]
    pub fn new() -> Self {
        Self {
            beating: Arc::new(AtomicBool::new(true)),
        }
    }

    #[must_use]
    pub fn is_beating(&self) -> bool {
        self.beating.load(Ordering::SeqCst)
    }

    pub fn suspend(&self) {
        self.beating.store(false, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.beating.store(true, Ordering::SeqCst);
    }
}

impl Default for Heartbeat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "tests/heartbeat.rs"]
mod tests;
