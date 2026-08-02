//! Observes one Gary turn from inside the session loop.
//!
//! The loop hands the recorder every model call and every tool call as they
//! happen, so coverage is structural: a surface wired into the loop cannot opt
//! out of being observed by forgetting a wrapper. Today the recorder only
//! trace-logs what it is given — nothing leaves the process.

use std::time::Duration;

use tracing::trace;

use super::llm::{ChatMessage, Completion};
use super::session::SessionOutcome;
use super::store::SessionId;
use super::turn::TurnIdentity;

/// Records one turn's model and tool activity against the turn's identity.
pub(crate) struct TurnRecorder {
    identity: TurnIdentity,
}

impl TurnRecorder {
    pub(crate) fn new(identity: TurnIdentity) -> Self {
        Self { identity }
    }

    /// Note that the loop is about to run, with the full identity it runs under.
    pub(crate) fn turn_started(&self) {
        let id = &self.identity;
        trace!(
            origin = ?id.origin,
            environment = ?id.environment(),
            guild = ?id.guild,
            channel = ?id.channel,
            actor = ?id.actor,
            access = ?id.access,
            session = session_field(id.session.as_ref()),
            "turn: started"
        );
    }

    /// One model call: what was sent, what came back, and what it cost.
    pub(crate) fn record_generation(&self, sent: &[ChatMessage], completion: &Completion) {
        trace!(
            session = session_field(self.identity.session.as_ref()),
            messages = sent.len(),
            model = completion.model,
            usage = ?completion.usage,
            finish_reason = ?completion.finish_reason,
            latency_ms = completion.latency.as_millis(),
            "turn: model call"
        );
    }

    /// One dispatched tool call, with the arguments and result the model saw.
    pub(crate) fn record_tool(
        &self,
        name: &str,
        arguments: &str,
        result: &str,
        duration: Duration,
    ) {
        trace!(
            session = session_field(self.identity.session.as_ref()),
            tool = name,
            arguments,
            result,
            duration_ms = duration.as_millis(),
            "turn: tool call"
        );
    }

    /// The turn reached a reply, whether by answering or by escalating.
    pub(crate) fn turn_finished(&self, outcome: &SessionOutcome) {
        trace!(
            session = session_field(self.identity.session.as_ref()),
            escalated = outcome.escalated,
            generations = outcome.generations.len(),
            reply_chars = outcome.reply.chars().count(),
            "turn: finished"
        );
    }

    /// The turn died on the model endpoint. Recorded separately from a finished
    /// turn so a provider outage stays visible instead of looking like silence.
    pub(crate) fn turn_failed(&self, error: &anyhow::Error) {
        trace!(
            session = session_field(self.identity.session.as_ref()),
            error = ?error,
            "turn: failed"
        );
    }
}

/// Render an optional session id for a log field, since a deferred batch has none.
fn session_field(session: Option<&SessionId>) -> &str {
    session.map_or("-", SessionId::as_str)
}
