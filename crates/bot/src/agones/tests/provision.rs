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

// ---- name-holder tenancy (issue #80) ----
//
// Instance names are Kubernetes object names, so they are unique across the whole
// namespace and one guild's choice can block another's. Which of the two happened
// decides what the friend is told: their own server by that name is worth naming,
// another guild's must not be confirmed to exist.

#[test]
fn a_name_held_by_the_asking_guild_is_theirs() {
    assert!(matches!(
        classify_holder(Some("111"), "111"),
        NameHolder::Caller
    ));
}

#[test]
fn a_name_held_by_another_guild_is_not_reported_as_theirs() {
    assert!(matches!(
        classify_holder(Some("222"), "111"),
        NameHolder::Other
    ));
}

#[test]
fn an_unlabelled_holder_is_never_the_caller() {
    // A pre-scoping object carries no guild label. Treating that as "yours" would
    // hand whoever asked first an instance nobody was recorded as owning.
    assert!(matches!(classify_holder(None, "111"), NameHolder::Other));
}

#[test]
fn a_guildless_caller_matches_nobody() {
    // The unscoped operator path passes an empty guild. Without the emptiness
    // guard it would compare equal to every unlabelled object.
    assert!(matches!(classify_holder(Some(""), ""), NameHolder::Other));
    assert!(matches!(classify_holder(None, ""), NameHolder::Other));
}
