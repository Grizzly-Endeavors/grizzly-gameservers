use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::llm::{ChatMessage, Completion, TokenUsage, ToolCall, ToolDef};
use super::recorder::TurnRecorder;

pub(crate) const DEFAULT_MAX_ROUNDS: usize = 16;

pub(crate) const ESCALATION_REPLY: &str =
    "I wasn't able to sort that out on my own — I've flagged it for Bear to take a look.";

pub(crate) const CANCELLED_REPLY: &str = "Stopped — I left everything as it was.";

const EMPTY_REPLY_FALLBACK: &str =
    "I finished, but didn't have anything to report back. Try asking again?";

pub(crate) type CompleteFn<'a> = dyn Fn(
        Vec<ChatMessage>,
        Vec<ToolDef>,
    ) -> Pin<Box<dyn Future<Output = Result<Completion>> + Send + 'a>>
    + Sync
    + 'a;

pub(crate) type DispatchFn<'a> =
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = String> + Send + 'a>> + Sync + 'a;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EscalationRequest {
    pub(crate) reply: String,
    pub(crate) reason: String,
    pub(crate) server: Option<String>,
}

pub(crate) type EscalationCheckFn<'a> = dyn Fn() -> Option<EscalationRequest> + Sync + 'a;

pub(crate) struct SessionCallbacks<'a, 'f> {
    pub(crate) complete: &'a CompleteFn<'f>,
    pub(crate) dispatch: &'a DispatchFn<'f>,
    pub(crate) progress: &'a ProgressFn<'f>,
    pub(crate) escalation_requested: &'a EscalationCheckFn<'f>,
    pub(crate) cancel: &'a CancellationToken,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EscalationReason {
    RoundBudget,
    AgentRequested {
        reason: String,
        server: Option<String>,
    },
}

impl EscalationReason {
    pub(crate) fn as_str(&self) -> &'static str {
        match *self {
            Self::RoundBudget => "round_budget_exhausted",
            Self::AgentRequested { .. } => "agent_requested",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SessionEvent {
    AssistantText(String),
}

pub(crate) type ProgressFn<'a> =
    dyn Fn(SessionEvent) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> + Sync + 'a;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnEnding {
    Answered,
    Escalated(EscalationReason),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOutcome {
    pub(crate) reply: String,
    pub(crate) ending: TurnEnding,
    pub(crate) generations: Vec<Generation>,
}

impl SessionOutcome {
    pub(crate) fn escalated(&self) -> bool {
        matches!(self.ending, TurnEnding::Escalated(_))
    }

    pub(crate) fn cancelled(&self) -> bool {
        matches!(self.ending, TurnEnding::Cancelled)
    }

    pub(crate) fn escalation(&self) -> Option<&EscalationReason> {
        match &self.ending {
            TurnEnding::Escalated(reason) => Some(reason),
            TurnEnding::Answered | TurnEnding::Cancelled => None,
        }
    }

    pub(crate) fn measured_prompt_tokens(&self) -> usize {
        self.generations
            .last()
            .and_then(|generation| generation.usage.as_ref())
            .and_then(|usage| usage.prompt)
            .map_or(0, |tokens| tokens as usize)
    }
}

pub(crate) async fn run_session(
    messages: &mut Vec<ChatMessage>,
    tools: Vec<ToolDef>,
    max_rounds: usize,
    recorder: &TurnRecorder,
    callbacks: &SessionCallbacks<'_, '_>,
) -> Result<SessionOutcome> {
    let complete = callbacks.complete;
    let dispatch = callbacks.dispatch;
    let progress = callbacks.progress;
    let escalation_requested = callbacks.escalation_requested;
    let cancel = callbacks.cancel;
    recorder.turn_started();
    let mut generations = Vec::new();

    for _ in 0..max_rounds {
        let completion = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let outcome = cancelled_outcome(messages, generations);
                recorder.turn_finished(&outcome);
                return Ok(outcome);
            }
            result = complete(messages.clone(), tools.clone()) => result,
        };
        let completion = match completion {
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
            messages.push(assistant);
            let outcome = SessionOutcome {
                reply,
                ending: TurnEnding::Answered,
                generations,
            };
            recorder.turn_finished(&outcome);
            return Ok(outcome);
        };

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

        if let Some(request) = escalation_requested() {
            let outcome = SessionOutcome {
                reply: request.reply,
                ending: TurnEnding::Escalated(EscalationReason::AgentRequested {
                    reason: request.reason,
                    server: request.server,
                }),
                generations,
            };
            recorder.turn_finished(&outcome);
            return Ok(outcome);
        }

        if cancel.is_cancelled() {
            let outcome = cancelled_outcome(messages, generations);
            recorder.turn_finished(&outcome);
            return Ok(outcome);
        }
    }

    let outcome = SessionOutcome {
        reply: ESCALATION_REPLY.to_owned(),
        ending: TurnEnding::Escalated(EscalationReason::RoundBudget),
        generations,
    };
    recorder.turn_finished(&outcome);
    Ok(outcome)
}

fn cancelled_outcome(
    messages: &mut Vec<ChatMessage>,
    generations: Vec<Generation>,
) -> SessionOutcome {
    messages.push(ChatMessage::assistant(CANCELLED_REPLY));
    SessionOutcome {
        reply: CANCELLED_REPLY.to_owned(),
        ending: TurnEnding::Cancelled,
        generations,
    }
}

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
