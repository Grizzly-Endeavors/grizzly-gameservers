use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::super::llm::{FunctionCall, Role};
use super::super::turn::{Environment, TurnActor, TurnIdentity, TurnOrigin};
use super::*;
use crate::discord::AccessLevel;

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

/// Wrap a canned assistant message as a completion, with token counts and a
/// latency standing in for what a provider would report.
fn completion(message: ChatMessage) -> Completion {
    completion_costing(message, 10)
}

fn completion_costing(message: ChatMessage, prompt_tokens: u32) -> Completion {
    Completion {
        message,
        model: "test-model".to_owned(),
        usage: Some(TokenUsage {
            prompt: Some(prompt_tokens),
            completion: Some(2),
            total: Some(prompt_tokens + 2),
        }),
        finish_reason: Some("stop".to_owned()),
        latency: Duration::from_millis(5),
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

/// A recorder for the loop to observe through. Its behavior is trace logging, so
/// the identity only has to be well-formed, not specific to a test.
fn recorder() -> TurnRecorder {
    let (telemetry, _poster) = crate::telemetry::Telemetry::new(
        None,
        "test".to_owned(),
        reqwest::Client::new(),
        tokio_util::sync::CancellationToken::new(),
    );
    TurnRecorder::new(
        TurnIdentity {
            origin: TurnOrigin::Discord,
            deployment: Environment::Production,
            guild: Some(1),
            channel: Some(2),
            actor: TurnActor::Discord(3),
            access: AccessLevel::Manager,
            session: None,
            game: None,
        },
        std::sync::Arc::new(telemetry),
    )
}

// Lifetime-parameterised to match `ProgressFn`, so a sink built here doesn't pin
// the whole callback bundle to `'static` and force the borrowing closures with it.
type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// A progress sink that discards every event.
fn ignore_progress<'f>() -> impl Fn(SessionEvent) -> ProgressFuture<'f> + Sync {
    |_| Box::pin(async {})
}

/// An escalation check for a surface whose tools never hand off, so the only
/// exits under test are a text reply and the round budget.
fn never_escalates() -> impl Fn() -> Option<EscalationRequest> + Sync {
    || None
}

#[tokio::test]
async fn returns_text_reply_without_calling_tools() {
    let turns = canned(vec![text_turn("3 servers are running")]);
    let dispatched = AtomicUsize::new(0);

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(completion(next)) })
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
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "3 servers are running");
    assert!(!outcome.escalated());
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
        Box::pin(async move { Ok(completion(next)) })
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
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "here they are");
    assert!(!outcome.escalated());
    assert_eq!(seen.lock().unwrap().as_slice(), ["list_servers"]);
    // The transcript is left populated for the caller to persist: the seed plus
    // the assistant tool turn, its tool result, and the final assistant text.
    assert!(
        messages.len() > 2,
        "the transcript should carry the turns the session ran"
    );
}

#[tokio::test]
async fn every_model_call_is_reported_in_order() {
    // Two rounds with different costs, so a caller can tell them apart and a
    // dropped or reordered entry can't pass.
    let turns = canned(vec![
        tool_turn("c1", "list_servers", "{}", None),
        text_turn("here they are"),
    ]);
    let round = AtomicUsize::new(0);

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        let prompt_tokens = if round.fetch_add(1, Ordering::SeqCst) == 0 {
            100
        } else {
            200
        };
        Box::pin(async move { Ok(completion_costing(next, prompt_tokens)) })
    };
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });
    let progress = ignore_progress();

    let mut messages = seed("list servers");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    let prompts: Vec<Option<u32>> = outcome
        .generations
        .iter()
        .map(|generation| generation.usage.as_ref().and_then(|usage| usage.prompt))
        .collect();
    assert_eq!(
        prompts,
        [Some(100), Some(200)],
        "one entry per model call, in the order they ran"
    );
    assert_eq!(
        outcome.generations.first().map(|g| g.latency),
        Some(Duration::from_millis(5)),
        "each call's measured latency rides along with its usage"
    );
}

#[tokio::test]
async fn an_escalating_turn_still_reports_what_it_spent() {
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Ok(completion(tool_turn("c", "list_servers", "{}", None))) });
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });
    let progress = ignore_progress();

    let mut messages = seed("loop forever");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        3,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    assert!(outcome.escalated());
    assert_eq!(
        outcome.escalation,
        Some(EscalationReason::RoundBudget),
        "running out of rounds is the budget exit, not an agent-requested one"
    );
    assert_eq!(
        outcome.generations.len(),
        3,
        "a turn that gave up still burned its whole budget, and that cost is real"
    );
}

/// An escalation check that fires once a tool has run, standing in for the tool
/// context slot the escalation tool writes to.
fn escalates_after_first_round(
    handed_off: &AtomicUsize,
) -> impl Fn() -> Option<EscalationRequest> + Sync {
    move || {
        (handed_off.load(Ordering::SeqCst) > 0).then(|| EscalationRequest {
            reply: "I can't fix this one — Bear is on it.".to_owned(),
            reason: "the world file is corrupt and needs a restore decision".to_owned(),
        })
    }
}

#[tokio::test]
async fn a_tool_escalation_ends_the_turn_with_the_models_own_reply() {
    // The model would happily keep calling tools forever; the escalation is the
    // only thing that can stop it before the budget does.
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Ok(completion(tool_turn("c", "escalate", "{}", None))) });
    let dispatched = AtomicUsize::new(0);
    let dispatch: &DispatchFn = &|_| {
        dispatched.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { "handed off".to_owned() })
    };
    let progress = ignore_progress();

    let mut messages = seed("the world is broken");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &escalates_after_first_round(&dispatched),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        outcome.reply, "I can't fix this one — Bear is on it.",
        "the user reads the model's own words, not the canned budget line"
    );
    assert_eq!(
        outcome.escalation,
        Some(EscalationReason::AgentRequested {
            reason: "the world file is corrupt and needs a restore decision".to_owned(),
        }),
        "the operator notice carries the reason, which is a different audience"
    );
    assert_eq!(
        outcome.generations.len(),
        1,
        "the turn stops on the escalating round rather than spending its budget"
    );
}

#[tokio::test]
async fn an_escalating_round_still_records_the_tools_it_ran_alongside() {
    let complete: &CompleteFn =
        &|_, _| Box::pin(async { Ok(completion(tool_turn("c", "read_logs", "{}", None))) });
    let dispatched = AtomicUsize::new(0);
    let dispatch: &DispatchFn = &|_| {
        dispatched.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { "nothing useful in the logs".to_owned() })
    };
    let progress = ignore_progress();

    let mut messages = seed("why is it down");
    run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &escalates_after_first_round(&dispatched),
        },
    )
    .await
    .unwrap();

    // Checked after the round, not mid-round: the operator notice is built from
    // the transcript, so a tool that ran beside the escalation has to be in it.
    let last = messages.last().unwrap();
    assert_eq!(last.role, Role::Tool);
    assert_eq!(
        last.content.as_deref(),
        Some("nothing useful in the logs"),
        "the round's tool results land in the transcript before the loop stops"
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
        Box::pin(async move { Ok(completion(next)) })
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
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
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
        tool_turn("c1", "destroy_server", "{}", Some("deleting minecraft now")),
        text_turn("done"),
    ]);
    let order: Mutex<Vec<&str>> = Mutex::new(Vec::new());

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(completion(next)) })
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
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
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
        &|_, _| Box::pin(async { Ok(completion(tool_turn("c", "list_servers", "{}", None))) });
    let dispatch: &DispatchFn = &|_| Box::pin(async { "ok".to_owned() });
    let progress = ignore_progress();

    let mut messages = seed("loop forever");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        3,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    assert!(outcome.escalated(), "an unbounded tool loop must escalate");
    assert_eq!(outcome.reply, ESCALATION_REPLY);
}

#[tokio::test]
async fn empty_text_reply_falls_back_to_a_prompt() {
    let turns = canned(vec![text_turn("   ")]);
    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(completion(next)) })
    };
    let dispatch: &DispatchFn = &|_| Box::pin(async { String::new() });
    let progress = ignore_progress();

    let mut messages = seed("hi");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap();

    assert!(!outcome.escalated());
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
        &recorder(),
        &SessionCallbacks {
            complete,

            dispatch,

            progress: &progress,

            escalation_requested: &never_escalates(),
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("unreachable"));
}
