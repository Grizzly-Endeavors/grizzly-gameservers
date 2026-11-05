//! The agent's tool-calling loop, kept free of Discord and Kubernetes so it is
//! testable with mock closures. The shell supplies two async callbacks —
//! `complete` (one model turn) and `dispatch` (run one tool) — and this drives
//! them: ask the model, run any tools it requests, feed results back, repeat
//! until it answers in plain text, a tool hands the turn to a human, a person
//! stops the turn, or the round budget is spent.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::llm::{ChatMessage, Completion, TokenUsage, ToolCall, ToolDef};
use super::recorder::TurnRecorder;

/// Default ceiling on model turns per request. The backstop behind the agent's
/// own escalation exit: a model that keeps calling tools without converging —
/// and without deciding to hand off — stops and hands back [`ESCALATION_REPLY`]
/// rather than thrashing. The shell records the give-up at `warn` and DMs the
/// configured operators (see `crate::notify`), so the "flagged for Bear" reply
/// is a promise the system keeps — the escalation sites in the Discord/in-game
/// shells do the notifying. Sized for a full troubleshooting cycle — browse,
/// read, edit, restart, check logs, and recover if needed — which spends several
/// tool rounds before a final reply.
pub(crate) const DEFAULT_MAX_ROUNDS: usize = 16;

/// What the user is told when the budget ran out before the agent reached an
/// answer *or* chose to hand off. An agent-requested escalation replaces this
/// with the model's own words, which is the point of having the tool.
pub(crate) const ESCALATION_REPLY: &str =
    "I wasn't able to sort that out on my own — I've flagged it for Bear to take a look.";

/// What lands in the transcript when a person stopped the turn. Short and
/// factual: whoever ran the command already got their own confirmation, so this
/// exists to keep the conversation coherent — the next turn must be able to see
/// that the previous request was abandoned rather than silently unanswered.
pub(crate) const CANCELLED_REPLY: &str = "Stopped — I left everything as it was.";

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

/// A tool's request to end the turn and put a human on it. The escalation tool
/// records one; the loop notices it on the next check and stops there, so the
/// model can't page an operator and then keep working on the same problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EscalationRequest {
    /// What the person who asked reads — the model's own closing words, in place
    /// of [`ESCALATION_REPLY`].
    pub(crate) reply: String,
    /// What the operator notice carries: the diagnosis and what's needed from a
    /// human. A different audience from `reply`, so a different field.
    pub(crate) reason: String,
    /// The instance this is about, when it is about one. Carried through to the
    /// notice and the event row so an operator can find it without re-reading
    /// the prose.
    pub(crate) server: Option<String>,
}

/// Consulted after each round of tool calls: `Some` means a tool asked for the
/// turn to end here. Synchronous because the answer is already recorded by the
/// time the tool returned — the loop is only reading a slot the shell owns.
pub(crate) type EscalationCheckFn<'a> = dyn Fn() -> Option<EscalationRequest> + Sync + 'a;

/// Everything the shell plugs into the loop. Bundled so [`run_session`]'s own
/// signature stays about the conversation — transcript, tools, budget — instead
/// of growing a parameter every time the loop learns to ask the shell something.
///
/// Two lifetimes, not one: `'a` is how long the bundle borrows the callbacks and
/// `'f` is how long each callback's own captures live. Collapsing them would
/// force a shell's closures to outlive the borrow, which none of them do — they
/// capture locals of the function that runs the turn.
pub(crate) struct SessionCallbacks<'a, 'f> {
    /// One model turn.
    pub(crate) complete: &'a CompleteFn<'f>,
    /// Run one tool call and render its result for the model.
    pub(crate) dispatch: &'a DispatchFn<'f>,
    /// Where the model's interim narration goes as it arrives.
    pub(crate) progress: &'a ProgressFn<'f>,
    /// Whether a tool has asked to end the turn; see [`EscalationCheckFn`].
    pub(crate) escalation_requested: &'a EscalationCheckFn<'f>,
    /// Tripped when a person asks for this turn to stop.
    ///
    /// The loop races it against each model call and re-checks it after each
    /// round of tools, so a stop lands on the model — which is where a runaway
    /// turn spends nearly all of its time — while a tool that is already running
    /// always finishes. That asymmetry is deliberate: abandoning a model call
    /// costs nothing but the tokens already spent, and abandoning a tool could
    /// leave a config half-written or a restart issued with nothing watching it
    /// come back.
    ///
    /// A surface with no way to stop a turn passes a token nobody holds, which
    /// simply never trips.
    pub(crate) cancel: &'a CancellationToken,
}

/// Why a turn ended without the agent answering the request itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EscalationReason {
    /// The loop spent its whole round budget without converging.
    RoundBudget,
    /// The model called the escalation tool — it judged that it could not finish
    /// and asked for a human, carrying the reason for the operator notice and
    /// the instance it named, if any.
    AgentRequested {
        reason: String,
        server: Option<String>,
    },
}

impl EscalationReason {
    /// Stable wire name, shared by the Langfuse categorical score and the
    /// product event log's outcome column so the two planes can be joined on it.
    /// The event-log constants (`events::mapping`) must match these exactly; a
    /// test there asserts it, since this module can't depend on that one.
    pub(crate) fn as_str(&self) -> &'static str {
        match *self {
            Self::RoundBudget => "round_budget_exhausted",
            Self::AgentRequested { .. } => "agent_requested",
        }
    }
}

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

/// How a turn finished.
///
/// An enum rather than a pair of flags because the three endings are mutually
/// exclusive and each shell has to do something different with each — post the
/// reply, page an operator, or say nothing because the person who stopped it was
/// already told. With `wildcard_enum_match_arm` denied, a fourth ending would
/// have to be answered at every one of those sites rather than falling into
/// whichever arm happened to be the catch-all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TurnEnding {
    /// The agent answered the request itself.
    Answered,
    /// The turn ended without an answer and a human is needed.
    Escalated(EscalationReason),
    /// A person stopped the turn while it was running. Not a failure and
    /// deliberately not an escalation: nobody needs to be paged, and counting it
    /// as one would inflate the escalation rate with turns the agent was never
    /// given the chance to finish.
    Cancelled,
}

/// The end state of a session: the text to send back, how it ended (so the
/// caller can log, notify, and score accordingly), and what each of its model
/// calls cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOutcome {
    pub(crate) reply: String,
    pub(crate) ending: TurnEnding,
    /// One entry per model call, in the order they ran.
    pub(crate) generations: Vec<Generation>,
}

impl SessionOutcome {
    /// Whether the turn ended by handing off rather than answering — the shape
    /// the telemetry score and the `escalation-rate` metric are defined on.
    pub(crate) fn escalated(&self) -> bool {
        matches!(self.ending, TurnEnding::Escalated(_))
    }

    /// Whether a person stopped this turn.
    pub(crate) fn cancelled(&self) -> bool {
        matches!(self.ending, TurnEnding::Cancelled)
    }

    /// Why the turn escalated, if it did.
    pub(crate) fn escalation(&self) -> Option<&EscalationReason> {
        match &self.ending {
            TurnEnding::Escalated(reason) => Some(reason),
            TurnEnding::Answered | TurnEnding::Cancelled => None,
        }
    }

    /// Prompt tokens the turn's largest request cost, as the provider counted
    /// them, or `0` if it reported none.
    ///
    /// The *last* generation rather than the largest by inspection: a turn's
    /// transcript only grows, so the final call is the biggest, and it is the one
    /// whose size the next turn inherits. A provider that reports no usage yields
    /// zero, which reads as "no reason to trim" — the safe direction, since the
    /// alternative is cutting a conversation on a number nobody measured.
    pub(crate) fn measured_prompt_tokens(&self) -> usize {
        self.generations
            .last()
            .and_then(|generation| generation.usage.as_ref())
            .and_then(|usage| usage.prompt)
            .map_or(0, |tokens| tokens as usize)
    }
}

/// Drive the tool-calling loop to a final reply.
///
/// Stops early if `callbacks.cancel` trips: the in-flight model call is
/// abandoned, any tool round already running is allowed to finish first, and the
/// transcript is closed with [`CANCELLED_REPLY`] so the conversation stays
/// coherent for the next turn.
///
/// `messages` is the seeded transcript — `[system, ...prior turns, user]` — and
/// is appended to in place as the model answers, so on return the caller holds
/// the full updated transcript to persist for the next turn. `progress` receives
/// the model's interim narration as it arrives. `recorder` observes every model
/// call and every tool call, so instrumentation coverage is a property of the
/// loop rather than of each shell remembering to wrap something. The shell's
/// callbacks arrive as one [`SessionCallbacks`]; a surface with no escalation
/// tool supplies a check that always answers `None`.
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
        // `messages` is untouched until the reply lands, so it still holds exactly
        // what was sent — no second clone needed to show the recorder the request.
        //
        // Biased so an already-tripped token wins without the model being asked
        // again: that is how a stop that arrived during the previous tool round
        // takes effect here, at the top of the round rather than part-way into a
        // call nobody is waiting for any more.
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
            // Record the answer in the transcript before returning, or the next
            // turn sees the user's question with no reply after it and answers it
            // again. Tool rounds already append their turns above; this is the one
            // exit that otherwise wouldn't.
            messages.push(assistant);
            let outcome = SessionOutcome {
                reply,
                ending: TurnEnding::Answered,
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

        // Checked after the round rather than after each call: a model that
        // escalates alongside other tool calls still gets those calls' results
        // into the transcript, so the operator notice reflects everything it
        // actually tried.
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

        // Checked here as well as at the top of the round so a stop that arrives
        // during the final round's tools is reported as a stop, rather than as
        // the round budget running out on its own.
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

/// Close out a turn a person stopped.
///
/// Appends the stop to the transcript for the same reason the plain-answer exit
/// appends its reply: without it the next turn sees the user's question with
/// nothing after it and answers it again — which on a turn somebody stopped
/// deliberately is the worst possible response.
///
/// Carries the generations already paid for, because a stopped turn still cost
/// whatever it spent getting there, and a cost report that omitted them would
/// understate exactly the turns most worth looking at.
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
