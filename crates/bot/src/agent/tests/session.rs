use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::super::llm::{FunctionCall, Role};
use super::super::turn::{Environment, TurnActor, TurnIdentity, TurnOrigin};
use super::*;
use crate::discord::AccessLevel;

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

fn text_turn(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: Some(text.to_owned()),
        tool_calls: None,
        tool_call_id: None,
    }
}

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
            prompt_details: None,
        }),
        finish_reason: Some("stop".to_owned()),
        latency: Duration::from_millis(5),
    }
}

fn seed(user: &str) -> Vec<ChatMessage> {
    vec![ChatMessage::system("sys"), ChatMessage::user(user)]
}

fn canned(turns: Vec<ChatMessage>) -> Mutex<VecDeque<ChatMessage>> {
    Mutex::new(VecDeque::from(turns))
}

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

type ProgressFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

fn ignore_progress<'f>() -> impl Fn(SessionEvent) -> ProgressFuture<'f> + Sync {
    |_| Box::pin(async {})
}

fn never_escalates() -> impl Fn() -> Option<EscalationRequest> + Sync {
    || None
}

fn never_cancelled() -> CancellationToken {
    CancellationToken::new()
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
            cancel: &never_cancelled(),
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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.reply, "here they are");
    assert!(!outcome.escalated());
    assert_eq!(seen.lock().unwrap().as_slice(), ["list_servers"]);
    assert!(
        messages.len() > 2,
        "the transcript should carry the turns the session ran"
    );
}

#[tokio::test]
async fn every_model_call_is_reported_in_order() {
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
            cancel: &never_cancelled(),
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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap();

    assert!(outcome.escalated());
    assert_eq!(
        outcome.escalation().cloned(),
        Some(EscalationReason::RoundBudget),
        "running out of rounds is the budget exit, not an agent-requested one"
    );
    assert_eq!(
        outcome.generations.len(),
        3,
        "a turn that gave up still burned its whole budget, and that cost is real"
    );
}

fn escalates_after_first_round(
    handed_off: &AtomicUsize,
) -> impl Fn() -> Option<EscalationRequest> + Sync {
    move || {
        (handed_off.load(Ordering::SeqCst) > 0).then(|| EscalationRequest {
            reply: "I can't fix this one — Bear is on it.".to_owned(),
            reason: "the world file is corrupt and needs a restore decision".to_owned(),
            server: Some("mc-summer".to_owned()),
        })
    }
}

#[tokio::test]
async fn a_tool_escalation_ends_the_turn_with_the_models_own_reply() {
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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        outcome.reply, "I can't fix this one — Bear is on it.",
        "the user reads the model's own words, not the canned budget line"
    );
    assert_eq!(
        outcome.escalation().cloned(),
        Some(EscalationReason::AgentRequested {
            reason: "the world file is corrupt and needs a restore decision".to_owned(),
            server: Some("mc-summer".to_owned()),
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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap();

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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        events.lock().unwrap().as_slice(),
        [SessionEvent::AssistantText(
            "let me check on that".to_owned()
        )]
    );
}

#[tokio::test]
async fn interim_text_is_delivered_before_the_tool_runs() {
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
            cancel: &never_cancelled(),
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
            cancel: &never_cancelled(),
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
            cancel: &never_cancelled(),
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
            cancel: &never_cancelled(),
        },
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("unreachable"));
}

#[tokio::test]
async fn a_turn_stopped_before_it_starts_never_calls_the_model() {
    let asked = AtomicUsize::new(0);
    let complete: &CompleteFn = &|_, _| {
        let asked = &asked;
        Box::pin(async move {
            asked.fetch_add(1, Ordering::SeqCst);
            Ok(completion(text_turn("hello")))
        })
    };
    let dispatch: &DispatchFn = &|_| Box::pin(async move { String::new() });
    let progress = ignore_progress();
    let cancel = CancellationToken::new();
    cancel.cancel();

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
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "the model call was never polled, so nothing was paid for"
    );
    assert_eq!(outcome.ending, TurnEnding::Cancelled);
    assert!(!outcome.escalated(), "a stop is not the agent giving up");
    assert!(outcome.cancelled());
}

#[tokio::test]
async fn a_stop_during_a_model_call_abandons_it() {
    let complete: &CompleteFn = &|_, _| Box::pin(std::future::pending());
    let dispatch: &DispatchFn = &|_| Box::pin(async move { String::new() });
    let progress = ignore_progress();
    let cancel = CancellationToken::new();

    let trip = cancel.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        trip.cancel();
    });

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
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.ending, TurnEnding::Cancelled);
}

#[tokio::test]
async fn a_tool_already_running_is_allowed_to_finish() {
    let turns = canned(vec![
        tool_turn("1", "restart_server", "{}", None),
        text_turn("never reached"),
    ]);
    let finished = AtomicUsize::new(0);
    let cancel = CancellationToken::new();

    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(completion(next)) })
    };
    let trip = cancel.clone();
    let dispatch: &DispatchFn = &|_| {
        let trip = trip.clone();
        let finished = &finished;
        Box::pin(async move {
            trip.cancel();
            tokio::task::yield_now().await;
            finished.fetch_add(1, Ordering::SeqCst);
            "restarted".to_owned()
        })
    };
    let progress = ignore_progress();

    let mut messages = seed("restart it");
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
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        finished.load(Ordering::SeqCst),
        1,
        "the tool ran to completion despite the stop arriving while it was in flight"
    );
    assert_eq!(outcome.ending, TurnEnding::Cancelled);
    assert!(
        messages
            .iter()
            .any(|message| message.role == Role::Tool
                && message.content.as_deref() == Some("restarted")),
        "the finished tool's result is still in the transcript"
    );
}

#[tokio::test]
async fn a_stopped_turn_records_the_stop_in_the_transcript() {
    let complete: &CompleteFn = &|_, _| Box::pin(std::future::pending());
    let dispatch: &DispatchFn = &|_| Box::pin(async move { String::new() });
    let progress = ignore_progress();
    let cancel = CancellationToken::new();
    cancel.cancel();

    let mut messages = seed("what's up");
    let before = messages.len();
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
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(messages.len(), before + 1);
    let last = messages.last().unwrap();
    assert_eq!(last.role, Role::Assistant);
    assert_eq!(last.content.as_deref(), Some(CANCELLED_REPLY));
    assert_eq!(outcome.reply, CANCELLED_REPLY);
}

#[tokio::test]
async fn a_stopped_turn_still_reports_what_it_spent() {
    let turns = canned(vec![
        tool_turn("1", "read_logs", "{}", None),
        text_turn("never reached"),
    ]);
    let cancel = CancellationToken::new();
    let complete: &CompleteFn = &|_, _| {
        let next = turns.lock().unwrap().pop_front().unwrap();
        Box::pin(async move { Ok(completion(next)) })
    };
    let trip = cancel.clone();
    let dispatch: &DispatchFn = &|_| {
        let trip = trip.clone();
        Box::pin(async move {
            trip.cancel();
            String::new()
        })
    };
    let progress = ignore_progress();

    let mut messages = seed("what happened");
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
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.ending, TurnEnding::Cancelled);
    assert_eq!(
        outcome.generations.len(),
        1,
        "the model call it already paid for is still counted"
    );
}

#[tokio::test]
async fn a_stop_on_the_last_round_reads_as_a_stop_not_a_spent_budget() {
    let cancel = CancellationToken::new();
    let complete: &CompleteFn =
        &|_, _| Box::pin(async move { Ok(completion(tool_turn("1", "read_logs", "{}", None))) });
    let trip = cancel.clone();
    let dispatch: &DispatchFn = &|_| {
        let trip = trip.clone();
        Box::pin(async move {
            trip.cancel();
            String::new()
        })
    };
    let progress = ignore_progress();

    let mut messages = seed("what happened");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        1,
        &recorder(),
        &SessionCallbacks {
            complete,
            dispatch,
            progress: &progress,
            escalation_requested: &never_escalates(),
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert_eq!(outcome.ending, TurnEnding::Cancelled);
    assert!(!outcome.escalated(), "nobody should be paged for a stop");
}

#[tokio::test]
async fn an_escalation_in_the_same_round_as_a_stop_still_pages() {
    let cancel = CancellationToken::new();
    let complete: &CompleteFn =
        &|_, _| Box::pin(async move { Ok(completion(tool_turn("1", "escalate", "{}", None))) });
    let trip = cancel.clone();
    let dispatch: &DispatchFn = &|_| {
        let trip = trip.clone();
        Box::pin(async move {
            trip.cancel();
            String::new()
        })
    };
    let progress = ignore_progress();
    let escalates = || {
        Some(EscalationRequest {
            reply: "Bear is on it.".to_owned(),
            reason: "needs a restore decision".to_owned(),
            server: None,
        })
    };

    let mut messages = seed("help");
    let outcome = run_session(
        &mut messages,
        Vec::new(),
        DEFAULT_MAX_ROUNDS,
        &recorder(),
        &SessionCallbacks {
            complete,
            dispatch,
            progress: &progress,
            escalation_requested: &escalates,
            cancel: &cancel,
        },
    )
    .await
    .unwrap();

    assert!(outcome.escalated());
    assert!(!outcome.cancelled());
}
