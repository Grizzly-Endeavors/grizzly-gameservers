use super::*;

const EVICTED_AT: i64 = 1_784_925_900;
const DWELL: Duration = Duration::from_mins(15);

fn meta_deleted_at(timestamp: &str) -> ObjectMeta {
    serde_json::from_value(serde_json::json!({
        "name": "palworld-wol9k",
        "deletionTimestamp": timestamp,
    }))
    .expect("metadata fixture should deserialize")
}

#[test]
fn a_pod_that_was_never_deleted_is_not_stranded() {
    assert!(!terminating_beyond(
        &ObjectMeta::default(),
        EVICTED_AT + 86_400,
        DWELL
    ));
}

#[test]
fn a_pod_still_inside_the_dwell_window_is_not_stranded() {
    let meta = meta_deleted_at("2026-07-24T20:45:00Z");
    assert!(!terminating_beyond(&meta, EVICTED_AT, DWELL));
    assert!(!terminating_beyond(&meta, EVICTED_AT + 120, DWELL));
    assert!(!terminating_beyond(&meta, EVICTED_AT + 899, DWELL));
}

#[test]
fn a_pod_past_the_dwell_window_is_stranded() {
    let meta = meta_deleted_at("2026-07-24T20:45:00Z");
    assert!(terminating_beyond(&meta, EVICTED_AT + 900, DWELL));
}

#[test]
fn the_outage_pod_reads_as_stranded_for_the_whole_outage() {
    let meta = meta_deleted_at("2026-07-24T20:45:00Z");
    let two_and_a_half_days = 60 * 60 * 60;
    assert!(terminating_beyond(
        &meta,
        EVICTED_AT + two_and_a_half_days,
        DWELL
    ));
}

#[test]
fn an_unusable_clock_strands_nothing() {
    let meta = meta_deleted_at("2026-07-24T20:45:00Z");
    assert!(!terminating_beyond(&meta, 0, DWELL));
}
