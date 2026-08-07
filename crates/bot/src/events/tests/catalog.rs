use super::*;

#[test]
fn surface_collapses_the_two_production_gary_origins() {
    assert_eq!(Surface::from_origin(TurnOrigin::Discord), Surface::Agent);
    assert_eq!(Surface::from_origin(TurnOrigin::Ingame), Surface::Agent);
    assert_eq!(
        Surface::from_origin(TurnOrigin::Deferred),
        Surface::Deferred
    );
    assert_eq!(
        Surface::from_origin(TurnOrigin::Operator),
        Surface::Operator
    );
}

#[test]
fn surface_names_are_the_published_vocabulary() {
    assert_eq!(Surface::Command.as_str(), "command");
    assert_eq!(Surface::Agent.as_str(), "agent");
    assert_eq!(Surface::Deferred.as_str(), "deferred");
    assert_eq!(Surface::Operator.as_str(), "operator");
    assert_eq!(Surface::System.as_str(), "system");
}

#[test]
fn event_kind_names_match_the_design_catalog() {
    let expected = [
        (EventKind::ServerCreated, "server_created"),
        (EventKind::ServerStarted, "server_started"),
        (EventKind::ServerStopped, "server_stopped"),
        (EventKind::ServerRestarted, "server_restarted"),
        (EventKind::ServerShutdown, "server_shutdown"),
        (EventKind::ServerDestroyed, "server_destroyed"),
        (EventKind::ServerArchived, "server_archived"),
        (EventKind::ServerRecovered, "server_recovered"),
        (EventKind::BackupTaken, "backup_taken"),
        (EventKind::ServerRestored, "server_restored"),
        (EventKind::BackupCycle, "backup_cycle"),
        (EventKind::Escalation, "escalation"),
        (EventKind::GuildJoined, "guild_joined"),
        (EventKind::BetaApproved, "beta_approved"),
        (EventKind::BetaDenied, "beta_denied"),
        (EventKind::SignupEmailed, "signup_emailed"),
    ];
    for (kind, name) in expected {
        assert_eq!(kind.as_str(), name);
    }
}

#[test]
fn a_player_actor_can_never_be_read_as_a_snowflake() {
    assert_eq!(Actor::Discord(4).label(), "4");
    assert_eq!(Actor::Player("4".to_owned()).label(), "player:4");
    assert_eq!(Actor::Player("system".to_owned()).label(), "player:system");
    assert_eq!(Actor::System.label(), "system");
}

#[test]
fn actor_carries_over_from_the_turn_that_asked() {
    assert_eq!(
        Actor::from_turn(&TurnActor::Discord(77)),
        Actor::Discord(77)
    );
    assert_eq!(
        Actor::from_turn(&TurnActor::Player("steve".to_owned())),
        Actor::Player("steve".to_owned())
    );
}

#[test]
fn an_event_starts_with_no_subject_and_empty_details() {
    let event = ProductEvent::new(EventKind::BackupCycle, &Attribution::system(), "completed");
    assert_eq!(event.actor, Actor::System);
    assert_eq!(event.surface, Surface::System);
    assert!(event.guild.is_none());
    assert!(event.game.is_none());
    assert!(event.instance.is_none());
    assert_eq!(event.details, serde_json::json!({}));
}

#[test]
fn builders_fill_in_the_subject() {
    let attribution = Attribution::new(Actor::Discord(9), Surface::Command);
    let event = ProductEvent::new(EventKind::ServerStopped, &attribution, "stopped")
        .guild(Some(GuildId::new("123")))
        .game(Some(GameId::new("minecraft")))
        .instance(InstanceName::new("mc-quiet-fox"))
        .details(serde_json::json!({ "size_bytes": 12 }));

    assert_eq!(event.guild, Some(GuildId::new("123")));
    assert_eq!(event.game, Some(GameId::new("minecraft")));
    assert_eq!(event.instance, Some(InstanceName::new("mc-quiet-fox")));
    assert_eq!(event.surface, Surface::Command);
    assert_eq!(event.details, serde_json::json!({ "size_bytes": 12 }));
}
