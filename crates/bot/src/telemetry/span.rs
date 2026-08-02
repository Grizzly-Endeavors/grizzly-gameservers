//! One finished Langfuse observation, and its translation to the wire type.
//!
//! Spans are assembled as [`SpanData`] literals rather than through a `Tracer`:
//! `SpanBuilder` cannot carry a caller-minted span id, and the SDK's own id
//! generator would break the parentage the recorder establishes up front. Going
//! straight to the processor also keeps the sampler and id generator out of the
//! path entirely, so nothing can override what was minted (see ADR-013).

use std::borrow::Cow;
use std::time::SystemTime;

use opentelemetry::trace::{
    SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
};
use opentelemetry::{InstrumentationScope, KeyValue};
use opentelemetry_sdk::trace::{SpanData, SpanEvents, SpanLinks};

/// A completed observation: one turn, one model call, or one tool call.
pub(crate) struct Observation {
    pub(crate) trace_id: TraceId,
    pub(crate) span_id: SpanId,
    /// `None` marks the trace root — the turn itself.
    pub(crate) parent: Option<SpanId>,
    pub(crate) name: Cow<'static, str>,
    pub(crate) start: SystemTime,
    pub(crate) end: SystemTime,
    pub(crate) status: Status,
    pub(crate) attributes: Vec<KeyValue>,
}

impl Observation {
    pub(crate) fn into_span_data(self, scope: InstrumentationScope) -> SpanData {
        SpanData {
            span_context: SpanContext::new(
                self.trace_id,
                self.span_id,
                // Nothing downstream re-samples, but an unsampled flag is how a
                // collector is told to discard — set it rather than inherit one.
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ),
            // The OTLP transform renders INVALID as an empty parent field, which
            // is what marks a span as its trace's root.
            parent_span_id: self.parent.unwrap_or(SpanId::INVALID),
            parent_span_is_remote: false,
            span_kind: SpanKind::Internal,
            name: self.name,
            start_time: self.start,
            end_time: self.end,
            attributes: self.attributes,
            dropped_attributes_count: 0,
            events: SpanEvents::default(),
            links: SpanLinks::default(),
            status: self.status,
            instrumentation_scope: scope,
        }
    }
}

#[cfg(test)]
#[path = "tests/span.rs"]
mod tests;
