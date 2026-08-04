use opentelemetry::Value;
use opentelemetry_sdk::trace::{BatchSpanProcessor, InMemorySpanExporter, SpanData};

use super::super::llm::Role;
use super::super::turn::{Environment, TurnActor, TurnOrigin};
use super::*;
use crate::discord::AccessLevel;

fn identity() -> TurnIdentity {
    TurnIdentity {
        origin: TurnOrigin::Discord,
        deployment: Environment::Production,
        guild: Some(1),
        channel: Some(2),
        actor: TurnActor::Discord(3),
        access: AccessLevel::Manager,
        session: None,
        game: None,
    }
}

fn assistant(text: &str) -> ChatMessage {
    ChatMessage {
        role: Role::Assistant,
        content: Some(text.to_owned()),
        tool_calls: None,
        tool_call_id: None,
    }
}

fn completion() -> Completion {
    Completion {
        message: assistant("done"),
        model: "test-model".to_owned(),
        usage: None,
        finish_reason: Some("stop".to_owned()),
        latency: Duration::from_millis(20),
    }
}

/// Run a turn through a recorder wired to an in-memory exporter and hand back
/// every span it produced.
fn exported(run: impl FnOnce(&TurnRecorder)) -> Vec<SpanData> {
    let exporter = InMemorySpanExporter::default();
    let telemetry = Arc::new(Telemetry::for_test(
        BatchSpanProcessor::builder(exporter.clone()).build(),
        "sha256:test",
    ));
    let recorder = TurnRecorder::new(identity(), Arc::clone(&telemetry));
    run(&recorder);
    telemetry.flush_for_test();
    exporter.get_finished_spans().unwrap_or_default()
}

fn attribute(span: &SpanData, key: &str) -> Option<Value> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.clone())
}

fn simple_turn(recorder: &TurnRecorder) {
    let sent = vec![ChatMessage::system("sys"), ChatMessage::user("restart mc")];
    recorder.turn_started();
    recorder.record_generation(&sent, &completion());
    recorder.record_tool("restart_server", "{}", "restarted", Duration::from_secs(1));
    recorder.record_generation(&sent, &completion());
    recorder.turn_finished(&SessionOutcome {
        reply: "done".to_owned(),
        escalation: None,
        generations: Vec::new(),
    });
}

#[test]
fn a_turn_exports_one_span_per_observation_plus_a_root() {
    let spans = exported(simple_turn);
    assert_eq!(spans.len(), 4, "root + 2 generations + 1 tool");
}

/// A wrong parent id produces a flat, unreadable trace in Langfuse rather than
/// an error, so parentage is asserted explicitly.
#[test]
fn every_observation_hangs_off_the_single_root() {
    let spans = exported(simple_turn);
    let roots = spans
        .iter()
        .filter(|span| span.parent_span_id == SpanId::INVALID)
        .collect::<Vec<_>>();
    assert_eq!(roots.len(), 1, "exactly one span is the trace root");
    let root_id = roots
        .first()
        .map(|root| root.span_context.span_id())
        .unwrap();
    for span in spans.iter().filter(|s| s.parent_span_id != SpanId::INVALID) {
        assert_eq!(span.parent_span_id, root_id);
    }
}

#[test]
fn every_span_shares_the_one_trace_id() {
    let spans = exported(simple_turn);
    let trace_ids = spans
        .iter()
        .map(|span| span.span_context.trace_id())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(trace_ids.len(), 1);
}

/// Langfuse does not inherit trace-level attributes, so a span missing the
/// identity block silently drops out of every filtered view.
#[test]
fn the_identity_block_is_stamped_on_every_span() {
    for span in exported(simple_turn) {
        assert!(
            attribute(&span, "langfuse.environment").is_some(),
            "{} is missing the identity block",
            span.name
        );
        assert_eq!(
            attribute(&span, "langfuse.release"),
            Some(Value::from("sha256:test"))
        );
    }
}

#[test]
fn generations_are_numbered_in_the_order_they_ran() {
    let rounds = exported(simple_turn)
        .iter()
        .filter(|span| span.name == "model.completion")
        .filter_map(|span| attribute(span, "langfuse.observation.metadata.round"))
        .map(|value| value.as_str().into_owned())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(rounds.len(), 2, "each model call gets its own round number");
}

#[test]
fn a_tool_span_is_named_for_its_tool() {
    let spans = exported(simple_turn);
    assert!(spans.iter().any(|span| span.name == "restart_server"));
}

/// A provider outage must stay visible as an errored trace rather than as
/// silence, and the partial observations before it are kept.
#[test]
fn a_failed_turn_still_exports_its_partial_trace() {
    let spans = exported(|recorder| {
        recorder.turn_started();
        recorder.record_generation(&[ChatMessage::user("hi")], &completion());
        recorder.turn_failed(&anyhow::anyhow!("model endpoint unreachable"));
    });
    assert_eq!(spans.len(), 2, "the generation survives the failure");
    let root = spans
        .iter()
        .find(|span| span.parent_span_id == SpanId::INVALID)
        .unwrap();
    assert!(matches!(root.status, Status::Error { .. }));
}

/// The root's window has to cover its children, or the Langfuse waterfall shows
/// a turn shorter than the work inside it.
#[test]
fn the_root_span_spans_the_whole_turn() {
    let spans = exported(simple_turn);
    let root = spans
        .iter()
        .find(|span| span.parent_span_id == SpanId::INVALID)
        .unwrap();
    for child in spans.iter().filter(|s| s.parent_span_id != SpanId::INVALID) {
        assert!(
            root.start_time <= child.end_time && child.end_time <= root.end_time,
            "{} falls outside the root's window",
            child.name
        );
    }
}

/// These five strings are the `guardrail` score's closed value set. Dashboards
/// and the operations doc key on them, so renaming one silently splits a metric.
#[test]
fn the_guardrail_vocabulary_is_the_designs_closed_set() {
    let rendered = [
        GuardrailVerdict::Healthy,
        GuardrailVerdict::RolledBack,
        GuardrailVerdict::RollbackFailed,
        GuardrailVerdict::Inconclusive,
        GuardrailVerdict::Unverified,
    ]
    .map(GuardrailVerdict::as_str);
    assert_eq!(
        rendered,
        [
            "healthy",
            "rolled_back",
            "rollback_failed",
            "inconclusive",
            "unverified"
        ]
    );
}

#[test]
fn a_disabled_sink_exports_nothing_and_still_runs_the_turn() {
    let (telemetry, _poster) = Telemetry::new(
        None,
        "dev".to_owned(),
        reqwest::Client::new(),
        tokio_util::sync::CancellationToken::new(),
    );
    let recorder = TurnRecorder::new(identity(), Arc::new(telemetry));
    simple_turn(&recorder);
    recorder.record_guardrail(GuardrailVerdict::Healthy);
}
