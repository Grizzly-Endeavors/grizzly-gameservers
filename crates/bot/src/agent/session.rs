//! The agent's tool-calling loop, kept free of Discord and Kubernetes so it is
//! testable with mock closures. The shell supplies two async callbacks —
//! `complete` (one model turn) and `dispatch` (run one tool) — and this drives
//! them: ask the model, run any tools it requests, feed results back, repeat
//! until it answers in plain text or the round budget is spent.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::llm::{ChatMessage, Completion, TokenUsage, ToolCall, ToolDef};
use super::recorder::TurnRecorder;

/// Default ceiling on model turns per request. A bounded loop is the lightweight
/// version of the design's escalation exit: a model that keeps calling tools
/// without converging stops and hands back [`ESCALATION_REPLY`] rather than
/// thrashing. The shell records the give-up at `warn` and DMs the configured
/// operators (see `crate::notify`), so the "flagged for Bear" reply is a promise
/// the system keeps — the escalation sites in the Discord/in-game shells do the
/// notifying. Sized for a full troubleshooting cycle — browse, read, edit,
/// restart, check logs, and recover if needed — which spends several tool rounds
/// before a final reply.
pub(crate) const DEFAULT_MAX_ROUNDS: usize = 16;

/// What the user is told the agent could not resolve on its own.
pub(crate) const ESCALATION_REPLY: &str =
    "I wasn't able to sort that out on my own — I've flagged it for Bear to take a look.";

/// Shown when the model returns neither tool calls nor any text.
const EMPTY_REPLY_FALLBACK: &str =
    "I finished, but didn't have anything to report back. Try asking again?";

/// One model turn: given the running transcript and the advertised tools, return
/// the assistant's next message along with what the call cost. Returning the
/// whole [`Completion`] rather than the bare message is what makes usage capture
/// structural — a shell can't drop it and still compile.
pub(crate) type CompleteFn<'a> = dyn Fn(
        Vec<ChatMessage>,
        Vec<ToolDef>,
    ) -> Pin<Box<dyn Future<Output = Result<Completion>> + Send + 'a>>
    + Sync
    + 'a;

/// Run one tool call and return the text result to feed back to the model.
pub(crate) type DispatchFn<'a> =
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> + Sync + 'a;

/// Something worth surfacing to the user mid-session, before the final reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    /// The model's own words on a tool-calling turn — narration it wrote
    /// alongside the tool calls (e.g. "let me restart minecraft for you"). Not a
    /// synthesized status line; only fires when the model actually says something.
    AssistantText(String),
}

/// Sink for [`SessionEvent`]s as the loop runs. Async and awaited inline so a
/// turn's narration is fully delivered before its tool calls run — the model's
/// "I'll restart it" must land ahead of the restart's side effects (e.g. a
/// confirmation prompt), not race them.
pub(crate) type ProgressFn<'a> =
    dyn Fn(SessionEvent) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Sync + 'a;

/// What one model call cost and how it ended. The message itself isn't repeated
/// here — it's already in the transcript the loop appended it to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Generation {
    pub(crate) model: String,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) finish_reason: Option<String>,
    pub(crate) latency: Duration,
}

impl From<&Completion> for Generation {
    fn from(completion: &Completion) -> Self {
        Self {
            model: completion.model.clone(),
            usage: completion.usage.clone(),
            finish_reason: completion.finish_reason.clone(),
            latency: completion.latency,
        }
    }
}

/// The end state of a session: the text to send back, whether it ended by
/// escalating (round budget exhausted) so the caller can log/flag accordingly,
/// and what each of its model calls cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOutcome {
    pub(crate) reply: String,
    pub(crate) escalated: bool,
    /// One entry per model call, in the order they ran.
    pub(crate) generations: Vec<Generation>,
}

/// Drive the tool-calling loop to a final reply.
///
/// `messages` is the seeded transcript — `[system, ...prior turns, user]` — and
/// is appended to in place as the model answers, so on return the caller holds
/// the full updated transcript to persist for the next turn. `progress` receives
/// the model's interim narration as it arrives. `recorder` observes every model
/// call and every tool call, so instrumentation coverage is a property of the
/// loop rather than of each shell remembering to wrap something.
///
/// # Errors
///
/// Returns an error only if `complete` itself errors (e.g. the endpoint is
/// unreachable). Tool failures are surfaced to the model as result text, not
/// propagated, so the model can react to them.
pub(crate) async fn run_session(
    messages: &mut Vec<ChatMessage>,
    tools: Vec<ToolDef>,
    max_rounds: usize,
    recorder: &TurnRecorder,
    complete: &CompleteFn<'_>,
    dispatch: &DispatchFn<'_>,
    progress: &ProgressFn<'_>,
) -> Result<SessionOutcome> {
    recorder.turn_started();
    let mut generations = Vec::new();

    for _ in 0..max_rounds {
        // `messages` is untouched until the reply lands, so it still holds exactly
        // what was sent — no second clone needed to show the recorder the request.
        let completion = match complete(messages.clone(), tools.clone()).await {
            Ok(completion) => completion,
            Err(err) => {
                recorder.turn_failed(&err);
                return Err(err);
            }
        };
        recorder.record_generation(messages, &completion);
        generations.push(Generation::from(&completion));
        let assistant = completion.message;

        let Some(calls) = assistant.requested_tool_calls().map(<[_]>::to_vec) else {
            let reply = assistant
                .content
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map_or_else(|| EMPTY_REPLY_FALLBACK.to_owned(), str::to_owned);
            // Record the answer in the transcript before returning, or the next
            // turn sees the user's question with no reply after it and answers it
            // again. Tool rounds already append their turns above; this is the one
            // exit that otherwise wouldn't.
            messages.push(assistant);
            let outcome = SessionOutcome {
                reply,
                escalated: false,
                generations,
            };
            recorder.turn_finished(&outcome);
            return Ok(outcome);
        };

        // Deliver any words the model wrote alongside its tool calls before we
        // run them — that narration is the only progress text the user sees, and
        // awaiting it here guarantees it lands ahead of the tools' side effects.
        if let Some(text) = assistant
            .content
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            progress(SessionEvent::AssistantText(text.to_owned())).await;
        }

        messages.push(assistant);
        run_tool_round(messages, calls, recorder, dispatch).await;
    }

    let outcome = SessionOutcome {
        reply: ESCALATION_REPLY.to_owned(),
        escalated: true,
        generations,
    };
    recorder.turn_finished(&outcome);
    Ok(outcome)
}

/// Run one round's tool calls in order, appending each result to the transcript
/// and timing it for the recorder.
async fn run_tool_round(
    messages: &mut Vec<ChatMessage>,
    calls: Vec<ToolCall>,
    recorder: &TurnRecorder,
    dispatch: &DispatchFn<'_>,
) {
    for call in calls {
        let id = call.id.clone();
        let name = call.function.name.clone();
        let arguments = call.function.arguments.clone();
        let started = Instant::now();
        let result = dispatch(call).await;
        recorder.record_tool(&name, &arguments, &result, started.elapsed());
        messages.push(ChatMessage::tool_result(id, result));
    }
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
