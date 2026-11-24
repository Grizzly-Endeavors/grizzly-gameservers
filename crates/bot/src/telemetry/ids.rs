use std::sync::atomic::{AtomicU64, Ordering};

use opentelemetry::trace::{SpanId, TraceId};
use tracing::warn;

static FALLBACK_COUNTER: AtomicU64 = AtomicU64::new(1);

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

fn non_zero<const N: usize>(mut bytes: [u8; N]) -> [u8; N] {
    if bytes.iter().all(|byte| *byte == 0) {
        bytes = [1_u8; N];
    }
    bytes
}

#[cfg(test)]
#[path = "tests/ids.rs"]
mod tests;
