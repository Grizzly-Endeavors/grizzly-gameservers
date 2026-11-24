use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::llm::{ChatMessage, Role};

const SESSION_TTL: Duration = Duration::from_mins(30);

pub(crate) const DEFAULT_TRANSCRIPT_TRIGGER_TOKENS: usize = 600_000;

pub(crate) const DEFAULT_TRANSCRIPT_TARGET_TOKENS: usize = 400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TranscriptLimits {
    pub(crate) trigger_tokens: usize,
    pub(crate) target_tokens: usize,
}

impl Default for TranscriptLimits {
    fn default() -> Self {
        Self {
            trigger_tokens: DEFAULT_TRANSCRIPT_TRIGGER_TOKENS,
            target_tokens: DEFAULT_TRANSCRIPT_TARGET_TOKENS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SessionKey {
    Channel(u64),
    IngamePlayer { guild: u64, player: u64 },
}

const SESSION_ID_BYTES: usize = 8;

static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SessionId(String);

impl SessionId {
    fn mint() -> Self {
        let mut high = [0_u8; SESSION_ID_BYTES];
        let mut low = [0_u8; SESSION_ID_BYTES];
        if let Err(err) = getrandom::fill(&mut high).and_then(|()| getrandom::fill(&mut low)) {
            let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
            warn!(
                error = ?err,
                "no system random source for a session id; falling back to a process counter"
            );
            return Self(format!("fallback{counter:016x}"));
        }
        Self(format!(
            "{:016x}{:016x}",
            u64::from_le_bytes(high),
            u64::from_le_bytes(low)
        ))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
}

pub(crate) struct SessionCheckout {
    pub(crate) id: SessionId,
    pub(crate) messages: Vec<ChatMessage>,
}

struct Entry {
    id: SessionId,
    messages: Vec<ChatMessage>,
    last_seen: Instant,
}

pub(crate) struct SessionStore {
    inner: Mutex<HashMap<SessionKey, Entry>>,
    turn_locks: Mutex<HashMap<SessionKey, Arc<tokio::sync::Mutex<()>>>>,
    ttl: Duration,
}

impl SessionStore {
    pub(crate) fn new() -> Self {
        Self::with_ttl(SESSION_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            turn_locks: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    pub(crate) async fn turn_lock(&self, key: SessionKey) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let Ok(mut locks) = self.turn_locks.lock() else {
                warn!("session turn-lock map is poisoned; this turn runs unserialized");
                return Arc::new(tokio::sync::Mutex::new(())).lock_owned().await;
            };
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(locks.entry(key).or_default())
        };
        lock.lock_owned().await
    }

    pub(crate) fn checkout(
        &self,
        key: SessionKey,
        now: Instant,
        fresh_system: impl FnOnce() -> String,
    ) -> SessionCheckout {
        if let Ok(store) = self.inner.lock()
            && let Some(entry) = store.get(&key)
            && now.duration_since(entry.last_seen) <= self.ttl
        {
            return SessionCheckout {
                id: entry.id.clone(),
                messages: entry.messages.clone(),
            };
        }
        SessionCheckout {
            id: SessionId::mint(),
            messages: vec![ChatMessage::system(fresh_system())],
        }
    }

    pub(crate) fn commit(
        &self,
        key: SessionKey,
        id: SessionId,
        messages: Vec<ChatMessage>,
        now: Instant,
        measured_tokens: usize,
        limits: TranscriptLimits,
    ) {
        let Ok(mut store) = self.inner.lock() else {
            return;
        };
        store.retain(|_, entry| now.duration_since(entry.last_seen) <= self.ttl);
        store.insert(
            key,
            Entry {
                id,
                messages: trim(messages, measured_tokens, limits),
                last_seen: now,
            },
        );
    }

    pub(crate) fn clear(&self, key: SessionKey) {
        if let Ok(mut store) = self.inner.lock() {
            store.remove(&key);
        }
    }
}

fn trim(
    messages: Vec<ChatMessage>,
    measured_tokens: usize,
    limits: TranscriptLimits,
) -> Vec<ChatMessage> {
    if measured_tokens <= limits.trigger_tokens {
        return messages;
    }
    let mut start = target_start(&messages, measured_tokens, limits.target_tokens);
    let last = messages.len().saturating_sub(1);
    while start < last && messages.get(start).is_some_and(|m| m.role == Role::Tool) {
        start += 1;
    }
    while start > 1 && messages.get(start).is_some_and(|m| m.role == Role::Tool) {
        start -= 1;
    }
    if start == 0 {
        return messages;
    }

    let mut trimmed = Vec::with_capacity(messages.len() - start + 1);
    if let Some(system) = messages.first() {
        trimmed.push(system.clone());
    }
    trimmed.extend(messages.into_iter().skip(start));
    trimmed
}

fn target_start(messages: &[ChatMessage], measured_tokens: usize, target_tokens: usize) -> usize {
    let total_bytes: usize = messages.iter().map(message_bytes).sum();
    let target_bytes = usize::try_from(
        (total_bytes as u128)
            .saturating_mul(target_tokens as u128)
            .checked_div(measured_tokens.max(1) as u128)
            .unwrap_or(0),
    )
    .unwrap_or(usize::MAX);

    let system_bytes = messages.first().map_or(0, message_bytes);
    let budget = target_bytes.saturating_sub(system_bytes);
    let mut used = 0_usize;
    let mut start = messages.len();
    for (index, message) in messages.iter().enumerate().skip(1).rev() {
        let bytes = message_bytes(message);
        if used > 0 && used.saturating_add(bytes) > budget {
            break;
        }
        used = used.saturating_add(bytes);
        start = index;
    }
    start
}

fn message_bytes(message: &ChatMessage) -> usize {
    let content = message.content.as_ref().map_or(0, String::len);
    let calls: usize = message
        .tool_calls
        .iter()
        .flatten()
        .map(|call| call.function.name.len() + call.function.arguments.len())
        .sum();
    content.saturating_add(calls)
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
