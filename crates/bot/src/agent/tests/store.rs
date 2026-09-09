use std::time::Duration;

use super::super::llm::Role;
use super::*;

const KEY: SessionKey = SessionKey::Channel(7);

fn roles(messages: &[ChatMessage]) -> Vec<Role> {
    messages.iter().map(|m| m.role).collect()
}

#[test]
fn fresh_key_starts_with_only_the_system_prompt() {
    let store = SessionStore::new();
    let now = Instant::now();

    let session = store.checkout(KEY, now, || "sys".to_owned());

    assert_eq!(roles(&session.messages), [Role::System]);
    assert_eq!(
        session.messages.first().and_then(|m| m.content.as_deref()),
        Some("sys")
    );
}

#[test]
fn checkout_continues_a_committed_transcript() {
    let store = SessionStore::new();
    let now = Instant::now();

    let session = store.checkout(KEY, now, || "sys".to_owned());
    let mut messages = session.messages;
    messages.push(ChatMessage::user("how's minecraft"));
    store.commit(
        KEY,
        session.id,
        messages,
        now,
        0,
        TranscriptLimits::default(),
    );

    let resumed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(roles(&resumed.messages), [Role::System, Role::User]);
    assert_eq!(
        resumed.messages.get(1).and_then(|m| m.content.as_deref()),
        Some("how's minecraft"),
        "the prior turn should carry forward"
    );
}

#[test]
fn an_idle_session_past_its_ttl_starts_fresh() {
    let ttl = Duration::from_mins(1);
    let store = SessionStore::with_ttl(ttl);
    let start = Instant::now();

    let session = store.checkout(KEY, start, || "sys".to_owned());
    let mut messages = session.messages;
    messages.push(ChatMessage::user("earlier"));
    store.commit(
        KEY,
        session.id,
        messages,
        start,
        0,
        TranscriptLimits::default(),
    );

    // Just within the window: still continued.
    let within = store.checkout(KEY, start + ttl, || "sys".to_owned());
    assert_eq!(
        within.messages.len(),
        2,
        "within the TTL the session resumes"
    );

    // Past the window: a clean slate.
    let expired = store.checkout(KEY, start + ttl + Duration::from_secs(1), || {
        "sys".to_owned()
    });
    assert_eq!(
        roles(&expired.messages),
        [Role::System],
        "an expired session resets"
    );
}

#[test]
fn clear_forgets_the_session() {
    let store = SessionStore::new();
    let now = Instant::now();

    let session = store.checkout(KEY, now, || "sys".to_owned());
    let mut messages = session.messages;
    messages.push(ChatMessage::user("hi"));
    store.commit(
        KEY,
        session.id,
        messages,
        now,
        0,
        TranscriptLimits::default(),
    );

    store.clear(KEY);

    let after = store.checkout(KEY, now, || "sys".to_owned());
    assert_eq!(
        roles(&after.messages),
        [Role::System],
        "cleared session is gone"
    );
}

/// Limits small enough to trip in a test without building a 600k-token
/// transcript. The ratio between them is what matters to the logic, not the
/// absolute values.
fn tight() -> TranscriptLimits {
    TranscriptLimits {
        trigger_tokens: 100,
        target_tokens: 40,
    }
}

/// A plausible measurement for `messages`, at the ~4 bytes/token the calibration
/// is meant to infer for itself.
fn measured(messages: &[ChatMessage]) -> usize {
    messages.iter().map(message_bytes).sum::<usize>() / 4
}

#[test]
fn a_transcript_under_the_trigger_is_never_cut() {
    // The whole point of the change: a long conversation is not a problem to be
    // solved. It is trimmed when it stops fitting and at no other time.
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..200 {
        messages.push(ChatMessage::user(format!("msg {i}")));
    }
    let count = messages.len();
    store.commit(
        KEY,
        SessionId::mint(),
        messages,
        now,
        99,
        TranscriptLimits {
            trigger_tokens: 100,
            target_tokens: 40,
        },
    );

    let kept = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        kept.messages.len(),
        count,
        "200 messages well inside the window must all survive"
    );
}

#[test]
fn passing_the_trigger_cuts_back_to_the_target_keeping_the_newest() {
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..60 {
        messages.push(ChatMessage::user(format!("message number {i}")));
    }
    let before = messages.len();
    let tokens = measured(&messages);
    store.commit(KEY, SessionId::mint(), messages, now, tokens, tight());

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    assert!(
        trimmed.messages.len() < before,
        "a transcript past the trigger is cut"
    );
    assert_eq!(
        trimmed.messages.first().map(|m| m.role),
        Some(Role::System),
        "the system prompt is always retained as the first message"
    );
    assert_eq!(
        trimmed.messages.last().and_then(|m| m.content.as_deref()),
        Some("message number 59"),
        "the most recent turn survives trimming"
    );
}

#[test]
fn a_cut_lands_near_the_target_not_just_under_the_trigger() {
    // Cutting only to the trigger would re-trim on the very next turn, and every
    // trim throws away the cached prompt prefix. The gap between the two bounds
    // is what makes a trim rare.
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..400 {
        messages.push(ChatMessage::user(format!("message number {i}")));
    }
    let tokens = measured(&messages);
    let limits = tight();
    store.commit(KEY, SessionId::mint(), messages, now, tokens, limits);

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    let retained_bytes: usize = trimmed.messages.iter().map(message_bytes).sum();
    // The same ~4 bytes/token the measurement above implies.
    let retained_tokens = retained_bytes / 4;
    assert!(
        retained_tokens <= limits.trigger_tokens,
        "a cut must at minimum get back under the trigger, got {retained_tokens}"
    );
    assert!(
        retained_tokens < limits.trigger_tokens,
        "a cut that only reaches the trigger would re-fire next turn"
    );
}

#[test]
fn trimming_never_begins_with_a_dangling_tool_result() {
    let store = SessionStore::new();
    let now = Instant::now();

    // A transcript long enough to trim whose natural cut point lands on tool
    // results — those must not lead the retained window.
    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..40 {
        messages.push(ChatMessage::tool_result(
            format!("call{i}"),
            format!("result {i}"),
        ));
    }
    messages.push(ChatMessage::user("latest"));
    // Measured far above the trigger, so the cut is deep enough to land inside
    // the run of tool results — which is the case being guarded.
    store.commit(KEY, SessionId::mint(), messages, now, 1_000, tight());

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        trimmed.messages.first().map(|m| m.role),
        Some(Role::System),
        "system stays first"
    );
    assert_ne!(
        trimmed.messages.get(1).map(|m| m.role),
        Some(Role::Tool),
        "the window after the system message must not open on a tool result"
    );
}

#[test]
fn a_live_session_keeps_its_id_across_turns() {
    let store = SessionStore::new();
    let now = Instant::now();

    let first = store.checkout(KEY, now, || "sys".to_owned());
    let id = first.id.clone();
    store.commit(
        KEY,
        first.id,
        first.messages,
        now,
        0,
        TranscriptLimits::default(),
    );

    let second = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        second.id, id,
        "one continuing conversation must stay one session downstream"
    );
}

#[test]
fn an_expired_session_is_a_new_session() {
    let ttl = Duration::from_mins(1);
    let store = SessionStore::with_ttl(ttl);
    let start = Instant::now();

    let first = store.checkout(KEY, start, || "sys".to_owned());
    let id = first.id.clone();
    store.commit(
        KEY,
        first.id,
        first.messages,
        start,
        0,
        TranscriptLimits::default(),
    );

    let revived = store.checkout(KEY, start + ttl + Duration::from_secs(1), || {
        "sys".to_owned()
    });
    assert_ne!(
        revived.id, id,
        "a transcript that timed out is a different conversation"
    );
}

#[test]
fn clear_retires_the_id() {
    let store = SessionStore::new();
    let now = Instant::now();

    let first = store.checkout(KEY, now, || "sys".to_owned());
    let id = first.id.clone();
    store.commit(
        KEY,
        first.id,
        first.messages,
        now,
        0,
        TranscriptLimits::default(),
    );
    store.clear(KEY);

    let after = store.checkout(KEY, now, || "sys".to_owned());
    assert_ne!(after.id, id, "an explicit reset starts a new session");
}

#[test]
fn separate_conversations_get_separate_ids() {
    let store = SessionStore::new();
    let now = Instant::now();

    let one = store.checkout(KEY, now, || "sys".to_owned());
    let other = store.checkout(SessionKey::Channel(8), now, || "sys".to_owned());

    assert_ne!(
        one.id, other.id,
        "two conversations must never share a session id"
    );
}

#[test]
fn a_channel_and_an_ingame_player_never_collide() {
    // The old tuple key relied on channel ids and player-name hashes happening
    // not to overlap. The variant makes that structural: even with identical
    // numbers, a channel is not a player.
    let store = SessionStore::new();
    let now = Instant::now();

    let channel = SessionKey::Channel(7);
    let player = SessionKey::IngamePlayer {
        guild: 7,
        player: 7,
    };
    assert_ne!(channel, player);

    let opened = store.checkout(channel, now, || "channel".to_owned());
    let mut messages = opened.messages;
    messages.push(ChatMessage::user("in the channel"));
    store.commit(
        channel,
        opened.id,
        messages,
        now,
        0,
        TranscriptLimits::default(),
    );

    let ingame = store.checkout(player, now, || "ingame".to_owned());
    assert_eq!(
        roles(&ingame.messages),
        [Role::System],
        "the player's conversation must not inherit the channel's transcript"
    );
}

#[tokio::test]
async fn turns_on_one_key_are_serialized() {
    // Two people mentioning Gary in the same channel at once is ordinary now, and
    // `commit` is last-write-wins — so the lock is what stops one of the two
    // exchanges vanishing along with the answer that was already posted.
    let store = std::sync::Arc::new(SessionStore::new());
    let held = store.turn_lock(KEY).await;

    let waiter = {
        let store = std::sync::Arc::clone(&store);
        tokio::spawn(async move { store.turn_lock(KEY).await })
    };
    tokio::task::yield_now().await;
    assert!(
        !waiter.is_finished(),
        "a second turn on the same key must wait for the first"
    );

    drop(held);
    waiter
        .await
        .expect("the queued turn runs once the first ends");
}

#[tokio::test]
async fn turns_on_different_keys_do_not_wait_for_each_other() {
    let store = SessionStore::new();
    let _held = store.turn_lock(KEY).await;

    // Another channel's conversation is unrelated; making it queue would serialize
    // the whole bot behind one slow turn.
    let other = store.turn_lock(SessionKey::Channel(8)).await;
    drop(other);
}

#[tokio::test]
async fn the_lock_map_does_not_grow_with_finished_turns() {
    let store = SessionStore::new();
    for channel in 0..50 {
        drop(store.turn_lock(SessionKey::Channel(channel)).await);
    }
    // Every guard is gone, so the next acquisition should sweep them all.
    let _held = store.turn_lock(SessionKey::Channel(99)).await;
    assert_eq!(
        store.turn_locks.lock().unwrap().len(),
        1,
        "only in-flight turns keep a lock entry alive"
    );
}

// ---- reaping and the byte budget (issue #83) ----

#[test]
fn committing_reaps_sessions_that_went_idle_past_the_ttl() {
    // Nothing else evicts: checkout only reads the TTL to decide freshness, so
    // without this an entry survives for the life of the process for every
    // channel Gary was ever mentioned in.
    let store = SessionStore::new();
    let start = Instant::now();
    let stale = SessionKey::Channel(1);
    let active = SessionKey::Channel(2);

    store.commit(
        stale,
        SessionId::mint(),
        vec![ChatMessage::system("s")],
        start,
        0,
        TranscriptLimits::default(),
    );
    let later = start + SESSION_TTL + Duration::from_secs(1);
    store.commit(
        active,
        SessionId::mint(),
        vec![ChatMessage::system("s")],
        later,
        0,
        TranscriptLimits::default(),
    );

    let inner = store.inner.lock().unwrap();
    assert_eq!(
        inner.len(),
        1,
        "the idle session should have been reaped by the later commit"
    );
    assert!(
        inner.contains_key(&active),
        "the session being committed must survive its own sweep"
    );
}

#[test]
fn a_live_session_is_not_reaped_by_another_keys_commit() {
    let store = SessionStore::new();
    let now = Instant::now();
    let other = SessionKey::Channel(9);

    store.commit(
        KEY,
        SessionId::mint(),
        vec![ChatMessage::system("s")],
        now,
        0,
        TranscriptLimits::default(),
    );
    store.commit(
        other,
        SessionId::mint(),
        vec![ChatMessage::system("s")],
        now,
        0,
        TranscriptLimits::default(),
    );

    assert_eq!(
        store.inner.lock().unwrap().len(),
        2,
        "sweeping must only drop sessions past the TTL"
    );
}

#[test]
fn a_huge_tool_result_no_longer_takes_the_conversation_with_it() {
    // The reported bug: one oversized read used to stop the backward walk, which
    // discarded that message *and every message older than it* — including the
    // user turns naming which server was being discussed.
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("look at the valheim server"),
    ];
    let big = "x".repeat(226_232);
    messages.push(ChatMessage::user("read the item config"));
    messages.push(ChatMessage::tool_result("call0", big));
    messages.push(ChatMessage::user("what is the stack size"));
    let tokens = measured(&messages);

    store.commit(
        KEY,
        SessionId::mint(),
        messages,
        now,
        tokens,
        TranscriptLimits::default(),
    );

    let kept = store.checkout(KEY, now, || "ignored".to_owned());
    assert!(
        kept.messages
            .iter()
            .filter_map(|m| m.content.as_deref())
            .any(|text| text.contains("valheim")),
        "the turn naming the server must outlive a large read of that server's files"
    );
}

#[test]
fn one_oversized_result_is_still_delivered() {
    // A single result larger than the whole target must survive: dropping it
    // would leave the model answering about a tool call it can no longer see.
    let store = SessionStore::new();
    let now = Instant::now();
    let huge = "x".repeat(64 * 1024);
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("read it"),
        ChatMessage::tool_result("call0", huge.clone()),
    ];
    let tokens = measured(&messages);
    store.commit(KEY, SessionId::mint(), messages, now, tokens, tight());

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        trimmed.messages.last().and_then(|m| m.content.as_deref()),
        Some(huge.as_str()),
        "the newest message is kept even when it alone exceeds the target"
    );
}

#[test]
fn a_small_transcript_is_untouched() {
    let store = SessionStore::new();
    let now = Instant::now();
    let messages = vec![
        ChatMessage::system("sys"),
        ChatMessage::user("hello"),
        ChatMessage::user("still here"),
    ];
    store.commit(
        KEY,
        SessionId::mint(),
        messages,
        now,
        0,
        TranscriptLimits::default(),
    );

    let kept = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(kept.messages.len(), 3, "nothing to trim");
}
