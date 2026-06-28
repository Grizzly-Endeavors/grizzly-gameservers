//! The agent's tool-calling loop, kept free of Discord and Kubernetes so it is
//! testable with mock closures. The shell supplies two async callbacks —
//! `complete` (one model turn) and `dispatch` (run one tool) — and this drives
//! them: ask the model, run any tools it requests, feed results back, repeat
//! until it answers in plain text or the round budget is spent.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use super::llm::{ChatMessage, ToolCall, ToolDef};

/// Default ceiling on model turns per request. A bounded loop is the lightweight
/// version of the design's escalation exit: a model that keeps calling tools
/// without converging is paged to a human rather than left to thrash.
pub(crate) const DEFAULT_MAX_ROUNDS: usize = 8;

/// What the user is told the agent could not resolve on its own.
pub(crate) const ESCALATION_REPLY: &str =
    "I wasn't able to sort that out on my own — I've flagged it for Bear to take a look.";

/// Shown when the model returns neither tool calls nor any text.
const EMPTY_REPLY_FALLBACK: &str =
    "I finished, but didn't have anything to report back. Try asking again?";

/// One model turn: given the running transcript and the advertised tools, return
/// the assistant's next message.
pub(crate) type CompleteFn<'a> = dyn Fn(
        Vec<ChatMessage>,
        Vec<ToolDef>,
    ) -> Pin<Box<dyn Future<Output = Result<ChatMessage>> + Send + 'a>>
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

/// Side-channel for [`SessionEvent`]s as the loop runs. Synchronous and
/// fire-and-forget so the shell can fan them out to Discord without blocking the
/// loop; the shell typically forwards them over a channel.
pub(crate) type ProgressFn<'a> = dyn Fn(SessionEvent) + Sync + 'a;

/// The end state of a session: the text to send back, and whether it ended by
/// escalating (round budget exhausted) so the caller can log/flag accordingly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionOutcome {
    pub(crate) reply: String,
    pub(crate) escalated: bool,
}

/// Drive the tool-calling loop to a final reply.
///
/// `messages` is the seeded transcript — `[system, ...prior turns, user]` — and
/// is appended to in place as the model answers, so on return the caller holds
/// the full updated transcript to persist for the next turn. `progress` receives
/// the model's interim narration as it arrives.
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
    complete: &CompleteFn<'_>,
    dispatch: &DispatchFn<'_>,
    progress: &ProgressFn<'_>,
) -> Result<SessionOutcome> {
    for _ in 0..max_rounds {
        let assistant = complete(messages.clone(), tools.clone()).await?;

        let Some(calls) = assistant.requested_tool_calls().map(<[_]>::to_vec) else {
            let reply = assistant
                .content
                .filter(|text| !text.trim().is_empty())
                .unwrap_or_else(|| EMPTY_REPLY_FALLBACK.to_owned());
            return Ok(SessionOutcome {
                reply,
                escalated: false,
            });
        };

        // Surface any words the model wrote alongside its tool calls before we go
        // run them — that narration is the only progress text the user sees.
        if let Some(text) = assistant
            .content
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            progress(SessionEvent::AssistantText(text.to_owned()));
        }

        messages.push(assistant);
        for call in calls {
            let id = call.id.clone();
            let result = dispatch(call).await;
            messages.push(ChatMessage::tool_result(id, result));
        }
    }

    Ok(SessionOutcome {
        reply: ESCALATION_REPLY.to_owned(),
        escalated: true,
    })
}

#[cfg(test)]
#[path = "tests/session.rs"]
mod tests;
