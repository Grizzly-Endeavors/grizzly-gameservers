use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::llm::{FunctionCall, Role};
use super::*;

/// An assistant turn that calls one tool, optionally with narration text.
fn tool_turn(id: &str, name: &str, arguments: &str, content: Option<&str>) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: content.map(str::to_owned),
        tool_calls: Some(vec![ToolCall {
            id: id.to_owned(),
            kind: "function".to_owned(),
            function: FunctionCall {
                name: name.to_owned(),
                arguments: arguments.to_owned(),
            },
        }]),
        tool_call_id: None,
    }
}

/// An assistant turn with plain text and no tool calls.
fn text_turn(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: Some(text.to_owned()),
        tool_calls: None,
        tool_call_id: None,
    }
}

/// The seeded transcript the shell hands to `run_session`.
fn seed(user: &str) -> Vec<ChatMessage> {
    vec![ChatMessage::system("sys"), ChatMessage::user(user)]
}

/// Build a `complete` closure that hands back the given canned turns in order.
fn canned(turns: Vec<ChatMessage>) -> Mutex<VecDeque<ChatMessage>> {
    Mutex::new(VecDeque::from(turns))
}

type ProgressFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// A progress sink that discards every event.
fn ignore_progress() -> impl Fn(SessionEvent) -> ProgressFuture + Sync {
    |_| Box::pin(async {})
}

#[tokio::test]
async fn returns_text_reply_without_calling_tools() {
    let turns = canned(vec![text_turn("3 servers are running")]);
    let dispatched = AtomicUsize::new(0);

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(next) })
    };
    let dispatch: &DispatchFn = &|_| {
        dispatched.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { String::new() })
    };
    let progress = ignore_progress();

    let mut messages = seed("what's up");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "3 servers are running");
    assert!(!outcome.escalated);
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        0,
        "a text-only reply must not invoke any tools"
    );
    // The final answer must land in the transcript the caller persists, or the
    // next turn would see the question with no answer after it and re-answer it.
    assert_eq!(
        messages.len(),
        3,
        "seed (system + user) plus the assistant reply"
    );
    let last = messages.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content.as_deref(), Some("3 servers are running"));
}

#[tokio::test]
async fn runs_one_tool_round_then_replies() {
    let turns = canned(vec![
        tool_turn("c1", "list_servers", "{}", None),
        text_turn("here they are"),
    ]);
    let seen = Mutex::new(Vec::new());

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(next) })
    };
    let dispatch: &DispatchFn = &|call: ToolCall| {
        seen.lock().unwrap().push(call.function.name.clone());
        Box::pin(async move { "mc-abc12: Ready".to_owned() })
    };
    let progress = ignore_progress();

    let mut messages = seed("list servers");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "here they are");
    assert!(!outcome.escalated);
    assert_eq!(seen.lock().unwrap().as_slice(), ["list_servers"]);
    // The transcript is left populated for the caller to persist: the seed plus
    // the assistant tool turn, its tool result, and the final assistant text.
    assert!(
        messages.len() > 2,
        "the transcript should carry the turns the session ran"
    );
}

#[tokio::test]
async fn emits_interim_text_only_when_the_model_narrates() {
    let turns = canned(vec![
        tool_turn("c1", "list_servers", "{}", Some("let me check on that")),
        tool_turn("c2", "list_servers", "{}", Some("   ")),
        tool_turn("c3", "list_servers", "{}", None),
        text_turn("done"),
    ]);
    let events: Mutex<Vec<SessionEvent>> = Mutex::new(Vec::new());

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(next) })
    };
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });
    let progress = |event: SessionEvent| {
        events.lock().unwrap().push(event);
        Box::pin(async {}) as ProgressFuture
    };

    let mut messages = seed("list servers");
    run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap();

    // Only the turn with real narration fires; blank and absent content do not.
    assert_eq!(
        events.lock().unwrap().as_slice(),
        [SessionEvent::AssistantText(
            "let me check on that".to_owned()
        )]
    );
}

#[tokio::test]
async fn interim_text_is_delivered_before_the_tool_runs() {
    // The bug this guards: narration that races its own tool's side effects (a
    // confirm card) and loses. The loop must await the progress send first.
    let turns = canned(vec![
        tool_turn("c1", "remove_server", "{}", Some("deleting minecraft now")),
        text_turn("done"),
    ]);
    let order: Mutex<Vec<&str>> = Mutex::new(Vec::new());

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(next) })
    };
    let dispatch: &DispatchFn = &|_| {
        order.lock().unwrap().push("dispatch");
        Box::pin(async { "ok".to_owned() })
    };
    let progress = |_: SessionEvent| {
        order.lock().unwrap().push("progress");
        Box::pin(async {}) as ProgressFuture
    };

    let mut messages = seed("delete minecraft");
    run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap();

    assert_eq!(
        order.lock().unwrap().as_slice(),
        ["progress", "dispatch"],
        "narration must be delivered before the tool it precedes runs"
    );
}

#[tokio::test]
async fn escalates_when_round_budget_is_exhausted() {
    // The model never stops calling tools — every turn is another call.
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Ok(tool_turn("c", "list_servers", "{}", None)) });
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });
    let progress = ignore_progress();

    let mut messages = seed("loop forever");
    let outcome = run_session(&mut messages, Vec::new(), 3, complete, dispatch, &progress)
        .await
        .unwrap();

    assert!(outcome.escalated, "an unbounded tool loop must escalate");
    assert_eq!(outcome.reply, ESCALATION_REPLY);
}

#[tokio::test]
async fn empty_text_reply_falls_back_to_a_prompt() {
    let turns = canned(vec![text_turn("   ")]);
    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(next) })
    };
    let dispatch: &DispatchFn = &|_| Box::pin(async { String::new() });
    let progress = ignore_progress();

    let mut messages = seed("hi");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap();

    assert!(!outcome.escalated);
    assert!(
        !outcome.reply.trim().is_empty(),
        "a blank model reply must not surface as an empty Discord message"
    );
}

#[tokio::test]
async fn propagates_completion_errors() {
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Err(anyhow::anyhow!("endpoint unreachable")) });
    let dispatch: &DispatchFn = &|_| Box::pin(async { String::new() });
    let progress = ignore_progress();

    let mut messages = seed("hi");
    let err = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
        &progress,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("unreachable"));
}
