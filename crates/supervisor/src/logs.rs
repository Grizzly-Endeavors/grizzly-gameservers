use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

pub const DEFAULT_TAIL_LINES: usize = 200;

const CAPACITY: usize = 1000;

#[derive(Debug)]
pub struct LogBuffer {
    lines: Mutex<VecDeque<String>>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl LogBuffer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(CAPACITY)),
        }
    }

    pub fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    #[must_use]
    pub fn tail(&self, count: usize) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        let start = lines.len().saturating_sub(count);
        lines.iter().skip(start).cloned().collect()
    }
}

#[cfg(test)]
#[path = "tests/logs.rs"]
mod tests;
