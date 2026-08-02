use super::*;

fn body(value: ScoreValue, name: &'static str) -> ScoreBody {
    ScoreBody::from(&ScorePost {
        trace_id: TraceId::from_bytes([1; 16]),
        name,
        value,
    })
}

/// Langfuse rejects JSON `true`/`false` for a boolean score — it wants 1/0.
#[test]
fn a_true_boolean_score_posts_as_one() {
    let rendered = body(ScoreValue::Boolean(true), "escalated");
    assert_eq!(rendered.value, serde_json::json!(1));
    assert_eq!(rendered.data_type, "BOOLEAN");
}

#[test]
fn a_false_boolean_score_posts_as_zero() {
    assert_eq!(
        body(ScoreValue::Boolean(false), "escalated").value,
        serde_json::json!(0)
    );
}

#[test]
fn a_categorical_score_posts_its_label_as_a_string() {
    let rendered = body(ScoreValue::Categorical("rolled_back"), "guardrail");
    assert_eq!(rendered.value, serde_json::json!("rolled_back"));
    assert_eq!(rendered.data_type, "CATEGORICAL");
}

/// The derived id is what makes a retry an upsert instead of a duplicate score.
#[test]
fn the_idempotency_id_combines_the_trace_and_the_score_name() {
    let rendered = body(ScoreValue::Boolean(true), "escalated");
    assert_eq!(rendered.id, format!("{}-escalated", rendered.trace_id));
}

#[test]
fn two_scores_on_one_trace_get_distinct_ids() {
    let escalated = body(ScoreValue::Boolean(true), "escalated");
    let guardrail = body(ScoreValue::Categorical("healthy"), "guardrail");
    assert_eq!(escalated.trace_id, guardrail.trace_id);
    assert_ne!(escalated.id, guardrail.id);
}

#[test]
fn the_body_serializes_with_the_field_names_langfuse_expects() {
    let json = serde_json::to_value(body(ScoreValue::Boolean(true), "escalated")).unwrap();
    let object = json.as_object().unwrap();
    let mut keys = object.keys().map(String::as_str).collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, ["dataType", "id", "name", "traceId", "value"]);
}
