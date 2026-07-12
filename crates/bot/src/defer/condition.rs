//! The pure core of the deferred-task queue: the [`Condition`] a task waits on,
//! the Redis key layout, the idle empty-streak transition, and the composition of
//! a fired batch into one prompt for Gary. No IO — all of this is unit-tested.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::task::DeferredTask;
use crate::prompts;

/// Every deferred-task key is namespaced with the app prefix (the shared kv-cache
/// requires it — isolation is by prefix, not ACL) and a `wait:` segment, then the
/// server name and condition: `gameservers:wait:{server}:{condition}`.
pub(crate) const KEY_PREFIX: &str = "gameservers:wait:";

/// A pattern matching every deferred-task key, for the startup `SCAN`.
pub(crate) const KEY_SCAN_PATTERN: &str = "gameservers:wait:*";

/// What a deferred task waits for. The model picks one; the watcher polls the
/// existing supervisor endpoints until it holds, then runs the queued tasks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Condition {
    /// The server has finished a (re)start — either it came up healthy and is
    /// accepting players, or it failed (crashed, boot-looping, or still not up
    /// after a long ceiling). Wakes Gary either way to report the outcome.
    Startup,
    /// The server has zero players connected right now — for changes wanted ASAP,
    /// as players are logging off.
    Empty,
    /// The server has been empty for a while (a grace window) — for no-rush tweaks
    /// that shouldn't fire the instant one person briefly disconnects.
    Idle,
}

impl Condition {
    /// The lowercase key segment / wire string for this condition.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::Empty => "empty",
            Self::Idle => "idle",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "startup" => Some(Self::Startup),
            "empty" => Some(Self::Empty),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }
}

/// The Redis list key for `(server, condition)`.
pub(crate) fn wait_key(server: &str, condition: Condition) -> String {
    format!("{KEY_PREFIX}{server}:{}", condition.as_str())
}

/// Recover `(server, condition)` from a key produced by [`wait_key`]. Returns
/// `None` for anything that isn't a well-formed deferred-task key — a stray key
/// under the prefix is skipped at reconciliation rather than spawning a bogus
/// watcher. Server names are k8s instance names (no colon), so the condition is
/// the segment after the final `:`.
pub(crate) fn parse_wait_key(key: &str) -> Option<(String, Condition)> {
    let rest = key.strip_prefix(KEY_PREFIX)?;
    let (server, condition) = rest.rsplit_once(':')?;
    if server.is_empty() {
        return None;
    }
    Some((server.to_owned(), Condition::from_str(condition)?))
}

/// The empty-streak transition for the `idle` condition: given the streak's
/// current start (or `None` if not currently streaking), the latest occupancy
/// reading, and now, return the new streak start. `Some(0)` continues (or starts)
/// the streak; anything else — players present, or an *unknown* count — resets it,
/// so a momentary blip or an unreadable console never counts toward idle.
pub(crate) fn next_empty_since<T: Copy>(
    current: Option<T>,
    reading: Option<u32>,
    now: T,
) -> Option<T> {
    match reading {
        Some(0) => current.or(Some(now)),
        _ => None,
    }
}

/// Frame a fired batch as one user turn for Gary. `trigger_note` describes what
/// woke the batch (e.g. "is now empty", or that a startup failed); the queued
/// tasks are listed for him to carry out. The system prompt is the normal
/// manager-tier Gary prompt, so this only supplies the situation and the asks.
pub(crate) fn compose_batch_prompt(
    server: &str,
    trigger_note: &str,
    tasks: &[DeferredTask],
) -> String {
    let listed = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| format!("{}. {}", index + 1, task.task.trim()))
        .collect::<Vec<_>>()
        .join("\n");
    prompts::DeferredBatch {
        server,
        trigger_note,
        tasks: &listed,
    }
    .render()
}

#[cfg(test)]
#[path = "tests/condition.rs"]
mod tests;
