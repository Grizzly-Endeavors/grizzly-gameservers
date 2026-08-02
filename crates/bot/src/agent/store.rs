//! In-memory conversation store giving Gary short-lived continuity: each
//! `(channel, user)` keeps its running transcript across mentions so "now restart
//! it" resolves, until it goes idle past the TTL or the user resets it. Nothing
//! is persisted — a restart wipes every session, by design (friends-scale, no
//! durable chat history to manage).

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tracing::warn;

use super::llm::{ChatMessage, Role};

/// How long a session may sit idle before its next use starts fresh.
const SESSION_TTL: Duration = Duration::from_mins(30);

/// Ceiling on retained messages per session (the leading system prompt always
/// counts as one and is never dropped). Bounds memory and prompt size for a
/// long-running back-and-forth; trimming keeps the most recent turns.
const MAX_TRANSCRIPT: usize = 30;

/// Identifies a conversation: `(channel_id, user_id)` as raw ids, keeping this
/// module free of Discord types (and trivially testable).
pub(crate) type SessionKey = (u64, u64);

/// How many random bytes back a session id. Two `u64`s of CSPRNG output are
/// collision-free at any volume this bot will ever see.
const SESSION_ID_BYTES: usize = 8;

/// Last-resort source of unique session ids if the system random source can't be
/// read. Telemetry must never be the reason a turn fails, so an unreadable
/// CSPRNG degrades to process-unique instead of erroring.
static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Opaque identifier for one conversation *instance*. Minted when a transcript
/// starts and retired with it, deliberately unrelated to [`SessionKey`] — so a
/// later change to how conversations are scoped can't silently re-key anything
/// downstream that grouped turns by this id.
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
}

/// A transcript handed out for one turn, with the id of the conversation
/// instance it belongs to. The id comes back on [`SessionStore::commit`].
pub(crate) struct SessionCheckout {
    pub(crate) id: SessionId,
    pub(crate) messages: Vec<ChatMessage>,
}

struct Entry {
    id: SessionId,
    messages: Vec<ChatMessage>,
    last_seen: Instant,
}

/// Keyed store of live conversation transcripts.
pub(crate) struct SessionStore {
    inner: Mutex<HashMap<SessionKey, Entry>>,
    ttl: Duration,
}

impl SessionStore {
    pub(crate) fn new() -> Self {
        Self::with_ttl(SESSION_TTL)
    }

    fn with_ttl(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    /// Hand back a clone of this key's transcript to run the next turn against,
    /// or a fresh `[system]` transcript when there's no live session (absent, or
    /// idle past the TTL). The lock is released before the caller awaits anything.
    ///
    /// A live session keeps its existing id; anything else mints a new one, so a
    /// conversation that timed out is a new conversation downstream too.
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

    /// Persist the transcript produced by a completed turn, trimmed to
    /// [`MAX_TRANSCRIPT`], under the id it was checked out with, and stamp it as
    /// just-used.
    pub(crate) fn commit(
        &self,
        key: SessionKey,
        id: SessionId,
        messages: Vec<ChatMessage>,
        now: Instant,
    ) {
        let Ok(mut store) = self.inner.lock() else {
            return;
        };
        store.insert(
            key,
            Entry {
                id,
                messages: trim(messages),
                last_seen: now,
            },
        );
    }

    /// Forget this key's session (the `/new-session` reset). A no-op if there
    /// isn't one.
    pub(crate) fn clear(&self, key: SessionKey) {
        if let Ok(mut store) = self.inner.lock() {
            store.remove(&key);
        }
    }
}

/// Keep the system message plus the most recent turns within [`MAX_TRANSCRIPT`].
/// The retained window must not begin with a tool result — that would dangle
/// without the assistant turn that requested it and malform the next request —
/// so the cut point advances past any leading tool messages.
fn trim(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    if messages.len() <= MAX_TRANSCRIPT {
        return messages;
    }

    let mut start = messages
        .len()
        .saturating_sub(MAX_TRANSCRIPT.saturating_sub(1));
    while messages.get(start).is_some_and(|m| m.role == Role::Tool) {
        start += 1;
    }

    let mut trimmed = Vec::with_capacity(messages.len() - start + 1);
    if let Some(system) = messages.first() {
        trimmed.push(system.clone());
    }
    trimmed.extend(messages.into_iter().skip(start));
    trimmed
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
