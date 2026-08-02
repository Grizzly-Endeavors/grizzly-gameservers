//! Trace and span id minting.
//!
//! Ids are minted here rather than by the OpenTelemetry SDK because the recorder
//! needs both before any span exists: the scores REST API is keyed on the trace
//! id, and a turn's generation/tool observations must name the root's span id
//! while the root itself is still running.

use std::sync::atomic::{AtomicU64, Ordering};

use opentelemetry::trace::{SpanId, TraceId};
use tracing::warn;

/// Last-resort source of unique ids if the system random source can't be read.
/// Telemetry must never be the reason a turn fails, so an unreadable CSPRNG
/// degrades to process-unique instead of erroring — the same bargain
/// `SessionId::mint` makes.
static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(1);

/// A fresh 16-byte trace id.
pub(crate) fn mint_trace_id() -> TraceId {
    let mut bytes = [0_u8; 16];
    if let Err(err) = getrandom::fill(&mut bytes) {
        let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
        warn!(
            error = ?err,
            "no system random source for a trace id; falling back to a process counter"
        );
        return TraceId::from_bytes(u128::from(counter).to_be_bytes());
    }
    TraceId::from_bytes(non_zero(bytes))
}

/// A fresh 8-byte span id.
pub(crate) fn mint_span_id() -> SpanId {
    let mut bytes = [0_u8; 8];
    if let Err(err) = getrandom::fill(&mut bytes) {
        let counter = FALLBACK_COUNTER.fetch_add(1, Ordering::Relaxed);
        warn!(
            error = ?err,
            "no system random source for a span id; falling back to a process counter"
        );
        return SpanId::from_bytes(counter.to_be_bytes());
    }
    SpanId::from_bytes(non_zero(bytes))
}

/// An all-zero id is the OpenTelemetry INVALID sentinel and is discarded silently
/// downstream, so trade the (vanishing) chance of it for a fixed byte.
fn non_zero<const N: usize>(mut bytes: [u8; N]) -> [u8; N] {
    if bytes.iter().all(|byte| *byte == 0) {
        bytes = [1_u8; N];
    }
    bytes
}

#[cfg(test)]
#[path = "tests/ids.rs"]
mod tests;
