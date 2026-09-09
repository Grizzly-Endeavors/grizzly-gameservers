//! Preserving a dead launch's output so a crash can still be explained.
//!
//! [`LogBuffer`](crate::logs::LogBuffer) serves the live tail, and for that job a
//! small ring is right. It is the wrong thing to diagnose a crash with, for three
//! reasons that all bite at once: it holds 1000 lines against a Valheim boot of
//! ~3700, so the *next* launch overwrites the evidence; the supervisor relaunches
//! a crashing child several times before escalating, stacking failed boots on top
//! of each other; and it lives in memory, so a pod recreation takes it entirely.
//!
//! The bot's auto-rollback makes that sharpest. A config change that crashes the
//! server is restored and restarted within seconds — which saves the world and
//! destroys the only account of what went wrong, so the agent is left reporting
//! that it rolled something back without being able to say why it had to.
//!
//! So each launch's output is captured separately and written to the instance's
//! volume when that launch dies unexpectedly. The volume outlives the pod, the
//! file outlives the next launch, and the agent reads it with the ordinary file
//! tools or greps it with the shell.
//!
//! **Head and tail, not a tail.** The two crash shapes put their evidence at
//! opposite ends: a mod that fails to load throws during startup (`BepInEx`
//! chainloads before the world is even up), while a server that dies after a week
//! says why in its last few lines. A plain ring would keep only the second. So
//! the capture keeps the first [`HEAD_LINES`] and the last [`TAIL_LINES`], and
//! says how many it dropped between them. The budget is sized so that an entire
//! ordinary boot — the case that actually matters — lands in the head with room
//! to spare and nothing is elided at all.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// Lines kept from the start of a launch. Comfortably more than a full modded
/// boot, because a startup failure is the shape that argued for keeping a head
/// at all.
const HEAD_LINES: usize = 3_000;

/// Lines kept from the end of a launch — where a long-running server that dies
/// says why.
const TAIL_LINES: usize = 12_000;

/// Directory on the instance's volume holding preserved launches. Under the
/// reserved `.grizzly/` tree, like the env overlay and the mod list: it is the
/// supervisor's to own, and the agent's to read.
const ARTIFACT_DIR: &str = ".grizzly/crashes";

/// How many preserved launches to retain. Enough to cover a crash loop's worth
/// of attempts without letting a wedged server fill its own volume.
const KEEP_ARTIFACTS: usize = 5;

/// One launch's captured output, bounded at both ends.
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
            // Stated rather than silent: a reader who can't see that the middle
            // is missing will draw conclusions from an ordering that isn't real.
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

/// The current launch's output. Cheap to clone behind an `Arc`; the line pumps
/// push and the runner takes, both under a short-lived lock never held across an
/// await.
/// How long [`LaunchLog::settle`] will wait for the output pumps to drain.
/// Generous against the milliseconds it actually takes, because the cost of
/// waiting too long is a slightly delayed rollback and the cost of not waiting
/// is losing the last lines — which are the ones that say why the child died.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(2);

/// Poll interval while draining. Short enough that the common case (pumps
/// already finished) costs one tick.
const SETTLE_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Default)]
pub struct LaunchLog {
    current: Mutex<Capture>,
    /// Output pumps still draining this launch's pipes. A child's exit and the
    /// arrival of its final lines are two different events, so the runner has to
    /// know when the second one has happened.
    open_pumps: AtomicUsize,
}

impl LaunchLog {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one line of the running child's output.
    pub fn push(&self, line: String) {
        self.lock().push(line);
    }

    /// Register an output pump that is about to start reading this launch.
    pub fn pump_started(&self) {
        self.open_pumps.fetch_add(1, Ordering::SeqCst);
    }

    /// Report a pump whose pipe has closed.
    pub fn pump_finished(&self) {
        self.open_pumps.fetch_sub(1, Ordering::SeqCst);
    }

    /// Wait, briefly, for every pump to finish draining.
    ///
    /// A child that dies leaves its last lines in the pipe, and those are the
    /// ones worth having. Returns `true` if the pumps drained, `false` if the
    /// wait timed out — in which case the capture is still written, just
    /// possibly missing its final lines, and the caller says so.
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

    /// Drop whatever the previous launch left behind. Called as each child is
    /// spawned, so a capture always describes exactly one launch — which is the
    /// property the shared live ring does not have.
    pub fn begin(&self) {
        *self.lock() = Capture::default();
    }

    /// The current launch's output as text, or `None` if it produced none.
    #[must_use]
    pub fn render(&self) -> Option<String> {
        let capture = self.lock();
        if capture.is_empty() {
            return None;
        }
        Some(capture.render())
    }

    /// A poisoned lock is recovered rather than propagated: a panic elsewhere
    /// must not be the reason a crash goes unexplained.
    fn lock(&self) -> std::sync::MutexGuard<'_, Capture> {
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Write `body` into the instance's crash directory and prune the oldest
/// artifacts past [`KEEP_ARTIFACTS`]. Returns the path written.
///
/// # Errors
///
/// Returns an error if the directory can't be created or the file can't be
/// written. Pruning failures are not errors — a preserved crash that couldn't be
/// tidied up after is still a preserved crash.
pub fn preserve(data_dir: &Path, body: &str) -> Result<PathBuf> {
    let dir = data_dir.join(ARTIFACT_DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let path = unique_path(&dir);
    std::fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    prune(&dir);
    Ok(path)
}

/// Ever-increasing suffix for artifact names, so a name is never reused within
/// the life of a supervisor.
///
/// Picking the first *free* sequence instead would reuse names, because pruning
/// frees the low ones it deletes: a crash loop inside a single second would keep
/// writing `-000`, and every artifact would collide with the name of one already
/// deleted. Monotonic here means write order and name order agree for any two
/// artifacts sharing a timestamp.
static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A path that doesn't exist yet. The timestamp is what a reader orders by; the
/// sequence only has to make the name unique among artifacts from the same
/// second, which a crash loop produces routinely.
fn unique_path(dir: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    // The existence check still stands behind the counter: a supervisor that
    // restarted inside the same second starts counting from zero again.
    for _ in 0..1000 {
        let sequence = NEXT_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let candidate = dir.join(format!("{stamp}-{sequence:03}.log"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stamp}-overflow.log"))
}

/// Keep the [`KEEP_ARTIFACTS`] most recently written files and delete the rest.
///
/// Ordered by modification time rather than by name. Name order cannot be
/// trusted here: pruning frees the low sequence numbers it deletes, so the next
/// crash in the same second reuses one and would sort as the oldest file present
/// the moment it was written — deleting the newest crash instead of the oldest.
/// The mtime is the fact being asked about anyway.
///
/// Best-effort: every failure is swallowed, because failing to prune must never
/// turn into failing to preserve. A file whose mtime can't be read sorts as
/// oldest, so an unreadable stat can cost an old artifact but never a new one.
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
