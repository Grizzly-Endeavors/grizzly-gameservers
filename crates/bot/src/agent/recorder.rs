//! Observes one Gary turn from inside the session loop.
//!
//! The loop hands the recorder every model call and every tool call as they
//! happen, so coverage is structural: a surface wired into the loop cannot opt
//! out of being observed by forgetting a wrapper.
//!
//! Trace and span ids are minted up front rather than assigned by the exporter.
//! That is what lets each observation ship the moment it finishes — a turn cut
//! short by a redeploy still leaves its model and tool calls behind — and it is
//! also what gives the scores API a trace to attach to before any span exists.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{SpanId, Status, TraceId};
use tracing::trace;

use super::llm::{ChatMessage, Completion};
use super::observations;
use super::session::SessionOutcome;
use super::store::SessionId;
use super::turn::TurnIdentity;
use crate::telemetry::{Observation, ScorePost, ScoreValue, Telemetry, ids};

/// Name of the boolean score carried by every turn.
const ESCALATED_SCORE: &str = "escalated";
/// Name of the categorical score carried by turns that restarted into a config
/// change.
const GUARDRAIL_SCORE: &str = "guardrail";

const ROOT_SPAN_NAME: &str = "gary.turn";
const GENERATION_SPAN_NAME: &str = "model.completion";

/// How a turn's config change came out, once the restart guardrail had its say.
/// A turn gets at most one of these — the final verdict; per-cycle history is
/// already visible in the turn's tool observations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuardrailVerdict {
    /// Came back up on the new config.
    Healthy,
    /// Crashed, and the automatic rollback recovered it.
    RolledBack,
    /// The rollback story ended badly — a second crash, a restore that didn't
    /// come back, or a restore that couldn't be issued at all.
    RollbackFailed,
    /// Verification ran but couldn't reach a verdict.
    Inconclusive,
    /// The restart path couldn't run verification at all.
    Unverified,
}

impl GuardrailVerdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::RolledBack => "rolled_back",
            Self::RollbackFailed => "rollback_failed",
            Self::Inconclusive => "inconclusive",
            Self::Unverified => "unverified",
        }
    }
}

/// Records one turn's model and tool activity against the turn's identity.
pub(crate) struct TurnRecorder {
    identity: TurnIdentity,
    telemetry: Arc<Telemetry>,
    trace_id: TraceId,
    /// Minted alongside the trace id so generation and tool observations can name
    /// their parent long before the root span is built.
    root_span_id: SpanId,
    /// Set once, by `turn_started`.
    started: OnceLock<SystemTime>,
    /// The question that opened the turn, lifted from the first request's
    /// transcript — the recorder is never handed it directly.
    opening: OnceLock<String>,
    rounds: AtomicU32,
    tools: AtomicUsize,
}

impl TurnRecorder {
    pub(crate) fn new(identity: TurnIdentity, telemetry: Arc<Telemetry>) -> Self {
        Self {
            identity,
            telemetry,
            trace_id: ids::mint_trace_id(),
            root_span_id: ids::mint_span_id(),
            started: OnceLock::new(),
            opening: OnceLock::new(),
            rounds: AtomicU32::new(0),
            tools: AtomicUsize::new(0),
        }
    }

    /// Note that the loop is about to run, with the full identity it runs under.
    pub(crate) fn turn_started(&self) {
        let id = &self.identity;
        self.started.get_or_init(SystemTime::now);
        trace!(
            trace_id = %self.trace_id,
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
        let round = self.rounds.fetch_add(1, Ordering::Relaxed) as usize + 1;
        if round == 1
            && let Some(opening) = observations::opening_input(sent)
        {
            self.opening.get_or_init(|| opening);
        }
        trace!(
            trace_id = %self.trace_id,
            session = session_field(self.identity.session.as_ref()),
            messages = sent.len(),
            model = completion.model,
            usage = ?completion.usage,
            finish_reason = ?completion.finish_reason,
            latency_ms = completion.latency.as_millis(),
            "turn: model call"
        );
        self.emit(
            GENERATION_SPAN_NAME.into(),
            completion.latency,
            Status::Ok,
            observations::generation(sent, completion, round),
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
        let index = self.tools.fetch_add(1, Ordering::Relaxed);
        trace!(
            trace_id = %self.trace_id,
            session = session_field(self.identity.session.as_ref()),
            tool = name,
            arguments,
            result,
            duration_ms = duration.as_millis(),
            "turn: tool call"
        );
        self.emit(
            name.to_owned().into(),
            duration,
            Status::Ok,
            observations::tool(arguments, result, index),
        );
    }

    /// The turn reached a reply, whether by answering or by escalating.
    pub(crate) fn turn_finished(&self, outcome: &SessionOutcome) {
        trace!(
            trace_id = %self.trace_id,
            session = session_field(self.identity.session.as_ref()),
            escalated = outcome.escalated,
            generations = outcome.generations.len(),
            reply_chars = outcome.reply.chars().count(),
            "turn: finished"
        );
        self.emit_root(
            Status::Ok,
            observations::root(self.opening.get().map(String::as_str), outcome),
        );
        self.telemetry.score(ScorePost {
            trace_id: self.trace_id,
            name: ESCALATED_SCORE,
            value: ScoreValue::Boolean(outcome.escalated),
            environment: self.identity.environment().as_str(),
        });
    }

    /// The turn died on the model endpoint. Recorded separately from a finished
    /// turn so a provider outage stays visible instead of looking like silence.
    /// The partial observations are kept and no outcome scores are attached — a
    /// provider failure is not an agent result.
    pub(crate) fn turn_failed(&self, error: &anyhow::Error) {
        let rendered = format!("{error:#}");
        trace!(
            trace_id = %self.trace_id,
            session = session_field(self.identity.session.as_ref()),
            error = ?error,
            "turn: failed"
        );
        self.emit_root(
            Status::error(rendered.clone()),
            observations::failed_root(self.opening.get().map(String::as_str), &rendered),
        );
    }

    /// How this turn's config change came out. Called by the shell after the loop
    /// returns, because the verdict is produced inside a tool, not by the loop.
    pub(crate) fn record_guardrail(&self, verdict: GuardrailVerdict) {
        trace!(
            trace_id = %self.trace_id,
            verdict = verdict.as_str(),
            "turn: guardrail verdict"
        );
        self.telemetry.score(ScorePost {
            trace_id: self.trace_id,
            name: GUARDRAIL_SCORE,
            value: ScoreValue::Categorical(verdict.as_str()),
            environment: self.identity.environment().as_str(),
        });
    }

    /// A child observation, timed backwards from now by the duration the loop
    /// already measured.
    fn emit(
        &self,
        name: Cow<'static, str>,
        duration: Duration,
        status: Status,
        mut attributes: Vec<opentelemetry::KeyValue>,
    ) {
        if !self.telemetry.is_enabled() {
            return;
        }
        let end = SystemTime::now();
        attributes.extend(observations::identity(
            &self.identity,
            self.telemetry.release(),
        ));
        self.telemetry.emit(Observation {
            trace_id: self.trace_id,
            span_id: ids::mint_span_id(),
            parent: Some(self.root_span_id),
            name,
            start: end.checked_sub(duration).unwrap_or(end),
            end,
            status,
            attributes,
        });
    }

    fn emit_root(&self, status: Status, mut attributes: Vec<opentelemetry::KeyValue>) {
        if !self.telemetry.is_enabled() {
            return;
        }
        let end = SystemTime::now();
        attributes.extend(observations::identity(
            &self.identity,
            self.telemetry.release(),
        ));
        self.telemetry.emit(Observation {
            trace_id: self.trace_id,
            span_id: self.root_span_id,
            parent: None,
            name: ROOT_SPAN_NAME.into(),
            start: self.started.get().copied().unwrap_or(end),
            end,
            status,
            attributes,
        });
    }
}

/// Render an optional session id for a log field, since a deferred batch has none.
fn session_field(session: Option<&SessionId>) -> &str {
    session.map_or("-", SessionId::as_str)
}

#[cfg(test)]
#[path = "tests/recorder.rs"]
mod tests;
