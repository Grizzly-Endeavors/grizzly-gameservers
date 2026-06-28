use std::time::Duration;

use super::super::llm::Role;
use super::*;

const KEY: SessionKey = (7, 42);

fn roles(messages: &[ChatMessage]) -> Vec<Role> {
    messages.iter().map(|m| m.role).collect()
}

#[test]
fn fresh_key_starts_with_only_the_system_prompt() {
    let store = SessionStore::new();
    let now = Instant::now();

    let messages = store.checkout(KEY, now, || "sys".to_owned());

    assert_eq!(roles(&messages), [Role::System]);
    assert_eq!(
        messages.first().and_then(|m| m.content.as_deref()),
        Some("sys")
    );
}

#[test]
fn checkout_continues_a_committed_transcript() {
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = store.checkout(KEY, now, || "sys".to_owned());
    messages.push(ChatMessage::user("how's minecraft"));
    store.commit(KEY, messages, now);

    let resumed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(roles(&resumed), [Role::System, Role::User]);
    assert_eq!(
        resumed.get(1).and_then(|m| m.content.as_deref()),
        Some("how's minecraft"),
        "the prior turn should carry forward"
    );
}

#[test]
fn an_idle_session_past_its_ttl_starts_fresh() {
    let ttl = Duration::from_mins(1);
    let store = SessionStore::with_ttl(ttl);
    let start = Instant::now();

    let mut messages = store.checkout(KEY, start, || "sys".to_owned());
    messages.push(ChatMessage::user("earlier"));
    store.commit(KEY, messages, start);

    // Just within the window: still continued.
    let within = store.checkout(KEY, start + ttl, || "sys".to_owned());
    assert_eq!(within.len(), 2, "within the TTL the session resumes");

    // Past the window: a clean slate.
    let expired = store.checkout(KEY, start + ttl + Duration::from_secs(1), || {
        "sys".to_owned()
    });
    assert_eq!(roles(&expired), [Role::System], "an expired session resets");
}

#[test]
fn clear_forgets_the_session() {
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = store.checkout(KEY, now, || "sys".to_owned());
    messages.push(ChatMessage::user("hi"));
    store.commit(KEY, messages, now);

    store.clear(KEY);

    let after = store.checkout(KEY, now, || "sys".to_owned());
    assert_eq!(roles(&after), [Role::System], "cleared session is gone");
}

#[test]
fn commit_trims_to_the_cap_keeping_the_system_message() {
    let store = SessionStore::new();
    let now = Instant::now();

    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..60 {
        messages.push(ChatMessage::user(format!("msg {i}")));
    }
    store.commit(KEY, messages, now);

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        trimmed.len(),
        MAX_TRANSCRIPT,
        "transcript is capped to MAX_TRANSCRIPT"
    );
    assert_eq!(
        trimmed.first().map(|m| m.role),
        Some(Role::System),
        "the system prompt is always retained as the first message"
    );
    assert_eq!(
        trimmed.last().and_then(|m| m.content.as_deref()),
        Some("msg 59"),
        "the most recent turn survives trimming"
    );
}

#[test]
fn trimming_never_begins_with_a_dangling_tool_result() {
    let store = SessionStore::new();
    let now = Instant::now();

    // A transcript long enough to trim whose natural cut point lands on tool
    // results — those must not lead the retained window.
    let mut messages = vec![ChatMessage::system("sys")];
    for i in 0..MAX_TRANSCRIPT {
        messages.push(ChatMessage::tool_result(
            format!("call{i}"),
            format!("result {i}"),
        ));
    }
    messages.push(ChatMessage::user("latest"));
    store.commit(KEY, messages, now);

    let trimmed = store.checkout(KEY, now, || "ignored".to_owned());
    assert_eq!(
        trimmed.first().map(|m| m.role),
        Some(Role::System),
        "system stays first"
    );
    assert_ne!(
        trimmed.get(1).map(|m| m.role),
        Some(Role::Tool),
        "the window after the system message must not open on a tool result"
    );
}
