//! In-memory conversation store giving Gary short-lived continuity: a
//! conversation keeps its running transcript across messages so "now restart it"
//! resolves, until it goes idle past the TTL or someone resets it. Nothing is
//! persisted — a restart wipes every session, by design (friends-scale, no
//! durable chat history to manage).
//!
//! What counts as one conversation is [`SessionKey`]: a whole Discord channel,
//! or a single in-game player. See its docs for why the two differ.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tracing::warn;

use super::llm::{ChatMessage, Role};

/// How long a session may sit idle before its next use starts fresh.
const SESSION_TTL: Duration = Duration::from_mins(30);

/// Ceiling on retained messages per session (the leading system prompt always
/// counts as one and is never dropped). Bounds memory and prompt size for a
/// long-running back-and-forth; trimming keeps the most recent turns.
const MAX_TRANSCRIPT: usize = 30;

/// Ceiling on retained transcript *bytes* per session, alongside the message
/// count.
///
/// A count alone doesn't bound anything that matters: a single `read_file`
/// result carries up to 256 KiB verbatim, so thirty of them is several megabytes
/// resident — and, worse, re-sent as prompt tokens on every later turn in that
/// channel. One big read should age out quickly rather than ride along for the
/// next twenty-nine messages. The newest message is always kept even when it
/// exceeds this on its own; dropping the result the model just asked for would
/// leave it answering about nothing.
const MAX_TRANSCRIPT_BYTES: usize = 128 * 1024;

/// What counts as one conversation. Raw ids rather than Discord types, so this
/// module stays trivially testable.
///
/// The two surfaces scope differently on purpose. A Discord channel is one
/// shared conversation: friends talk to Gary in front of each other, and an
/// admin has to be able to answer a question or approve an action that someone
/// else set going. In-game chat stays per-player — a player has no tier, gets a
/// fixed read-only tool set, and there is nothing there for a second person to
/// take part in.
///
/// Spelled as an enum rather than a pair of ids so the two key spaces can't
/// collide by construction; the previous tuple relied on channel ids and player
/// name hashes happening not to overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SessionKey {
    /// Every message in one Discord channel (or DM), whoever sent it.
    Channel(u64),
    /// One player's in-game conversation on the servers of one guild.
    IngamePlayer { guild: u64, player: u64 },
}

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

    /// A fixed id for tests. Minting stays private so the store remains the only
    /// source of real ids.
    #[cfg(test)]
    pub(crate) fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
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
    /// One turn lock per key; see [`SessionStore::turn_lock`].
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

    /// Serialize turns on one key. Hold the guard across checkout → run →
    /// commit.
    ///
    /// [`commit`](Self::commit) is last-write-wins, so two turns running against
    /// the same key interleave into one transcript surviving and the other being
    /// silently discarded — including whichever answer the model already posted.
    /// With per-user keys that took two messages from the same person within one
    /// turn; with a channel key it takes two people talking at once, which is
    /// ordinary. Queuing is the right trade: the second asker waits a few seconds
    /// and gets an answer that has the first exchange in view.
    ///
    /// The lock map self-prunes: an entry is only kept while a guard is
    /// outstanding, so idle keys leave nothing behind.
    pub(crate) async fn turn_lock(&self, key: SessionKey) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = {
            let Ok(mut locks) = self.turn_locks.lock() else {
                // A poisoned map would mean a panic while holding it. Serializing
                // is a safety property, not a correctness one for a single turn,
                // so degrade to an unshared lock rather than failing the turn.
                warn!("session turn-lock map is poisoned; this turn runs unserialized");
                return Arc::new(tokio::sync::Mutex::new(())).lock_owned().await;
            };
            locks.retain(|_, lock| Arc::strong_count(lock) > 1);
            Arc::clone(locks.entry(key).or_default())
        };
        lock.lock_owned().await
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
        // Reap here rather than from a timer. `checkout` only *reads* the TTL to
        // decide freshness, so without this an entry persists for the life of the
        // process for every channel Gary was ever mentioned in and every
        // (guild, player) pair that ever typed @Gary in game — none of which ever
        // come back. Sweeping on write costs one pass over the live keys at the
        // one moment the lock is already held, and mirrors how `turn_locks`
        // self-prunes rather than adding a task that itself needs supervising.
        store.retain(|_, entry| now.duration_since(entry.last_seen) <= self.ttl);
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

/// Keep the system message plus the most recent turns within [`MAX_TRANSCRIPT`]
/// and [`MAX_TRANSCRIPT_BYTES`].
///
/// The retained window must not begin with a tool result — that would dangle
/// without the assistant turn that requested it and malform the next request —
/// so the cut point advances past any leading tool messages, after both limits
/// have had their say.
fn trim(messages: Vec<ChatMessage>) -> Vec<ChatMessage> {
    let mut start = messages
        .len()
        .saturating_sub(MAX_TRANSCRIPT.saturating_sub(1));
    start = start.max(byte_budget_start(&messages));
    // Skip leading tool results, which would dangle without the assistant turn
    // that asked for them — but never past the newest message, or a transcript
    // whose last message *is* a tool result would be trimmed away entirely.
    let last = messages.len().saturating_sub(1);
    while start < last && messages.get(start).is_some_and(|m| m.role == Role::Tool) {
        start += 1;
    }
    // Landing on a tool result now means it is the newest message, so walk back
    // to the assistant turn that requested it and keep the pair intact. This can
    // exceed the byte budget; a well-formed request matters more than the budget,
    // and the alternative is a call with no visible result.
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

/// The earliest message index (after the system prompt) that keeps the retained
/// tail within [`MAX_TRANSCRIPT_BYTES`].
///
/// Walks backwards from the newest so the most recent exchange is what survives,
/// and always admits at least one message — a single oversized result still has
/// to reach the model that asked for it. The system prompt is excluded from the
/// walk because it is never dropped; it is charged against the budget so a large
/// prompt doesn't leave the tail unbounded.
fn byte_budget_start(messages: &[ChatMessage]) -> usize {
    let system_bytes = messages.first().map_or(0, message_bytes);
    let budget = MAX_TRANSCRIPT_BYTES.saturating_sub(system_bytes);
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

/// What one message costs the transcript: its content plus the arguments of any
/// tool calls it carries, which are re-sent with it and are not always small.
/// Near enough for a budget — bounding growth is the point, not exactness.
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
