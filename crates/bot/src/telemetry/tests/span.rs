use std::time::Duration;

use super::*;

fn scope() -> InstrumentationScope {
    InstrumentationScope::builder("test").build()
}

fn observation(parent: Option<SpanId>) -> Observation {
    let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    Observation {
        trace_id: TraceId::from_bytes([7; 16]),
        span_id: SpanId::from_bytes([3; 8]),
        parent,
        name: "gary.turn".into(),
        start,
        end: start + Duration::from_millis(250),
        status: Status::Ok,
        attributes: vec![KeyValue::new("langfuse.environment", "operator")],
    }
}

#[test]
fn a_parentless_observation_is_the_trace_root() {
    let data = observation(None).into_span_data(scope());
    assert_eq!(data.parent_span_id, SpanId::INVALID);
}

#[test]
fn a_child_observation_keeps_its_parent() {
    let parent = SpanId::from_bytes([9; 8]);
    let data = observation(Some(parent)).into_span_data(scope());
    assert_eq!(data.parent_span_id, parent);
}

#[test]
fn minted_ids_survive_into_the_span_context() {
    let data = observation(None).into_span_data(scope());
    assert_eq!(data.span_context.trace_id(), TraceId::from_bytes([7; 16]));
    assert_eq!(data.span_context.span_id(), SpanId::from_bytes([3; 8]));
}

/// An unsampled span is discarded by collectors, so the flag has to be set
/// explicitly — there is no tracer in this path to set it.
#[test]
fn spans_are_marked_sampled() {
    let data = observation(None).into_span_data(scope());
    assert!(data.span_context.is_sampled());
}

#[test]
fn timestamps_and_attributes_pass_through_verbatim() {
    let source = observation(None);
    let (start, end) = (source.start, source.end);
    let data = source.into_span_data(scope());
    assert_eq!(data.start_time, start);
    assert_eq!(data.end_time, end);
    assert_eq!(data.attributes.len(), 1);
}
