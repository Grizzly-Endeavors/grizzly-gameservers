use super::*;

/// 2026-07-24T20:45:00Z — the instant the outage's pod was evicted.
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

// A normal shutdown terminates inside its grace period. Forcing pods out during
// that window would cut the world-save short, which is the opposite of the point.
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

// The actual outage: the pod sat Terminating from 07-24 until the node came back
// on 07-27. Every pass over those two and a half days should have recovered it.
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

// A clock that reads before the epoch yields 0, which must defer recovery rather
// than make everything look infinitely stranded and force pods out at random.
#[test]
fn an_unusable_clock_strands_nothing() {
    let meta = meta_deleted_at("2026-07-24T20:45:00Z");
    assert!(!terminating_beyond(&meta, 0, DWELL));
}
