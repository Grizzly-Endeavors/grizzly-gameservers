use serde::{Deserialize, Serialize};

use super::task::DeferredTask;
use crate::prompts;
use crate::prompts::RunWhenCondition;

pub(crate) const KEY_PREFIX: &str = "gameservers:wait:";

pub(crate) const KEY_SCAN_PATTERN: &str = "gameservers:wait:*";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Condition {
    Startup,
    Empty,
    Idle,
}

impl Condition {
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

impl From<RunWhenCondition> for Condition {
    fn from(value: RunWhenCondition) -> Self {
        match value {
            RunWhenCondition::Startup => Self::Startup,
            RunWhenCondition::Empty => Self::Empty,
            RunWhenCondition::Idle => Self::Idle,
        }
    }
}

pub(crate) fn wait_key(server: &str, condition: Condition) -> String {
    format!("{KEY_PREFIX}{server}:{}", condition.as_str())
}

pub(crate) fn parse_wait_key(key: &str) -> Option<(String, Condition)> {
    let rest = key.strip_prefix(KEY_PREFIX)?;
    let (server, condition) = rest.rsplit_once(':')?;
    if server.is_empty() {
        return None;
    }
    Some((server.to_owned(), Condition::from_str(condition)?))
}

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
