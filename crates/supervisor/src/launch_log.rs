use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

const HEAD_LINES: usize = 3_000;

const TAIL_LINES: usize = 12_000;

const ARTIFACT_DIR: &str = ".grizzly/crashes";

const KEEP_ARTIFACTS: usize = 5;

#[derive(Debug, Default)]
struct Capture {
    head: Vec<String>,
    tail: VecDeque<String>,
    elided: usize,
}

impl Capture {
    fn push(&mut self, line: String) {
        if self.head.len() < HEAD_LINES {
            self.head.push(line);
            return;
        }
        if self.tail.len() == TAIL_LINES {
            self.tail.pop_front();
            self.elided = self.elided.saturating_add(1);
        }
        self.tail.push_back(line);
    }

    fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.head {
            out.push_str(line);
            out.push('\n');
        }
        if self.elided > 0 {
            out.push_str("... ");
            out.push_str(&self.elided.to_string());
            out.push_str(" lines elided between the start and end of this launch ...\n");
        }
        for line in &self.tail {
            out.push_str(line);
            out.push('\n');
        }
        out
    }

    fn is_empty(&self) -> bool {
        self.head.is_empty() && self.tail.is_empty()
    }
}

const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

const SETTLE_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
pub struct LaunchLog {
    current: Mutex<Capture>,
    open_pumps: AtomicUsize,
}

impl LaunchLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, line: String) {
        self.lock().push(line);
    }

    pub fn pump_started(&self) {
        self.open_pumps.fetch_add(1, Ordering::SeqCst);
    }

    pub fn pump_finished(&self) {
        self.open_pumps.fetch_sub(1, Ordering::SeqCst);
    }

    pub async fn settle(&self) -> bool {
        let deadline = tokio::time::Instant::now() + SETTLE_TIMEOUT;
        while self.open_pumps.load(Ordering::SeqCst) > 0 {
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
        true
    }

    pub fn begin(&self) {
        *self.lock() = Capture::default();
    }

    #[must_use]
    pub fn render(&self) -> Option<String> {
        let capture = self.lock();
        if capture.is_empty() {
            return None;
        }
        Some(capture.render())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Capture> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub fn preserve(data_dir: &Path, body: &str) -> Result<PathBuf> {
    let dir = data_dir.join(ARTIFACT_DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = unique_path(&dir);
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    prune(&dir);
    Ok(path)
}

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn unique_path(dir: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    for _ in 0..1000 {
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let candidate = dir.join(format!("{stamp}-{sequence:03}.log"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stamp}-overflow.log"))
}

fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut artifacts: Vec<(SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok().map(|found| found.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "log"))
        .map(|path| {
            let written = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .unwrap_or(UNIX_EPOCH);
            (written, path)
        })
        .collect();
    if artifacts.len() <= KEEP_ARTIFACTS {
        return;
    }
    artifacts.sort();
    let excess = artifacts.len().saturating_sub(KEEP_ARTIFACTS);
    for (_, path) in artifacts.iter().take(excess) {
        drop(std::fs::remove_file(path));
    }
}

#[cfg(test)]
#[path = "tests/launch_log.rs"]
mod tests;
