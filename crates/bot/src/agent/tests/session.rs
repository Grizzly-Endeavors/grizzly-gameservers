#![expect(clippy::unwrap_used, reason = "test code uses unwrap for clarity")]

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::llm::{FunctionCall, Role};
use super::*;

/// An assistant turn that calls one tool.
fn tool_turn(id: &str, name: &str, arguments: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: None,
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

/// Build a `complete` closure that hands back the given canned turns in order.
fn canned(turns: Vec<ChatMessage>) -> Mutex<VecDeque<ChatMessage>> {
    Mutex::new(VecDeque::from(turns))
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

    let outcome = run_session(
        "sys".to_owned(),
        "what's up".to_owned(),
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
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
}

#[tokio::test]
async fn runs_one_tool_round_then_replies() {
    let turns = canned(vec![
        tool_turn("c1", "list_servers", "{}"),
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

    let outcome = run_session(
        "sys".to_owned(),
        "list servers".to_owned(),
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "here they are");
    assert!(!outcome.escalated);
    assert_eq!(seen.lock().unwrap().as_slice(), ["list_servers"]);
}

#[tokio::test]
async fn escalates_when_round_budget_is_exhausted() {
    // The model never stops calling tools — every turn is another call.
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Ok(tool_turn("c", "list_servers", "{}")) });
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });

    let outcome = run_session(
        "sys".to_owned(),
        "loop forever".to_owned(),
        Vec::new(),
        3,
        complete,
        dispatch,
    )
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

    let outcome = run_session(
        "sys".to_owned(),
        "hi".to_owned(),
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
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

    let err = run_session(
        "sys".to_owned(),
        "hi".to_owned(),
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        complete,
        dispatch,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("unreachable"));
}
