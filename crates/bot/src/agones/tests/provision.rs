use super::*;

use super::super::types::{GameServerSpec, GameServerStatus};

// Built through serde rather than by hand: this is exactly how metadata reaches
// the bot from the API server, timestamp parsing included.
fn meta_from_json(value: serde_json::Value) -> ObjectMeta {
    serde_json::from_value(value).expect("metadata fixture should deserialize")
}

fn gameserver_in_state(state: Option<&str>) -> GameServer {
    let mut gameserver = GameServer::new("world", GameServerSpec { container: None });
    gameserver.status = Some(GameServerStatus {
        state: state.map(str::to_owned),
    });
    gameserver
}

#[test]
fn metadata_without_a_deletion_timestamp_is_live() {
    assert_eq!(deletion_state(&ObjectMeta::default()), DeletionState::Live);
}

#[test]
fn metadata_with_a_deletion_timestamp_is_terminating() {
    let meta = meta_from_json(serde_json::json!({
        "name": "palworld-wol9k",
        "deletionTimestamp": "2026-07-24T20:45:00Z",
    }));
    assert_eq!(deletion_state(&meta), DeletionState::Terminating);
}

// The distinction the outage turned on: a delete Kubernetes has *accepted*
// leaves the object in the API wearing a deletion timestamp and its finalizer,
// which is emphatically not "gone".
#[test]
fn an_accepted_delete_held_by_a_finalizer_is_not_live() {
    let meta = meta_from_json(serde_json::json!({
        "name": "palworld-wol9k",
        "deletionTimestamp": "2026-07-24T20:45:00Z",
        "finalizers": ["agones.dev/controller"],
    }));
    assert_eq!(deletion_state(&meta), DeletionState::Terminating);
}

#[test]
fn ready_and_allocated_gameservers_are_ready() {
    assert!(is_ready(&gameserver_in_state(Some("Ready"))));
    assert!(is_ready(&gameserver_in_state(Some("Allocated"))));
}

#[test]
fn pre_ready_and_unhealthy_gameservers_are_not_ready() {
    assert!(!is_ready(&gameserver_in_state(Some("Scheduled"))));
    assert!(!is_ready(&gameserver_in_state(Some("Unhealthy"))));
    assert!(!is_ready(&gameserver_in_state(None)));
}
