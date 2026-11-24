use std::borrow::Cow;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use opentelemetry::trace::{SpanId, Status, TraceId};
use tracing::trace;

use super::llm::{ChatMessage, Completion};
use super::observations;
use super::session::{EscalationReason, SessionOutcome};
use super::store::SessionId;
use super::turn::TurnIdentity;
use crate::telemetry::{Observation, ScorePost, ScoreValue, Telemetry, ids};

const ESCALATED_SCORE: &str = "escalated";
const ESCALATION_KIND_SCORE: &str = "escalation_kind";
const CANCELLED_SCORE: &str = "cancelled";
const GUARDRAIL_SCORE: &str = "guardrail";

const ROOT_SPAN_NAME: &str = "gary.turn";
const GENERATION_SPAN_NAME: &str = "model.completion";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuardrailVerdict {
    Healthy,
    RolledBack,
    RollbackFailed,
    Inconclusive,
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

pub(crate) struct TurnRecorder {
    identity: TurnIdentity,
    telemetry: Arc<Telemetry>,
    trace_id: TraceId,
    root_span_id: SpanId,
    started: OnceLock<SystemTime>,
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

    pub(crate) fn turn_finished(&self, outcome: &SessionOutcome) {
        trace!(
            trace_id = %self.trace_id,
            session = session_field(self.identity.session.as_ref()),
            escalated = outcome.escalated(),
            cancelled = outcome.cancelled(),
            escalation_kind = outcome.escalation().map(EscalationReason::as_str),
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
            value: ScoreValue::Boolean(outcome.escalated()),
            environment: self.identity.environment().as_str(),
        });
        self.telemetry.score(ScorePost {
            trace_id: self.trace_id,
            name: CANCELLED_SCORE,
            value: ScoreValue::Boolean(outcome.cancelled()),
            environment: self.identity.environment().as_str(),
        });
        if let Some(reason) = outcome.escalation() {
            self.telemetry.score(ScorePost {
                trace_id: self.trace_id,
                name: ESCALATION_KIND_SCORE,
                value: ScoreValue::Categorical(reason.as_str()),
                environment: self.identity.environment().as_str(),
            });
        }
    }

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

fn session_field(session: Option<&SessionId>) -> &str {
    session.map_or("-", SessionId::as_str)
}

#[cfg(test)]
#[path = "tests/recorder.rs"]
mod tests;
