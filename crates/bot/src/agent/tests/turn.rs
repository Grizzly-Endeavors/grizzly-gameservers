use super::*;

fn identity(origin: TurnOrigin, deployment: Environment) -> TurnIdentity {
    TurnIdentity {
        origin,
        deployment,
        guild: Some(7),
        channel: Some(42),
        actor: TurnActor::Discord(99),
        access: AccessLevel::Manager,
        session: None,
        game: None,
    }
}

#[test]
fn operator_turns_are_quarantined_wherever_they_run() {
    for deployment in [
        Environment::Production,
        Environment::Development,
        Environment::Operator,
    ] {
        assert_eq!(
            identity(TurnOrigin::Operator, deployment).environment(),
            Environment::Operator,
            "an operator turn must never be counted as {deployment:?} traffic"
        );
    }
}

#[test]
fn every_other_origin_inherits_the_deployment() {
    for origin in [
        TurnOrigin::Discord,
        TurnOrigin::Ingame,
        TurnOrigin::Deferred,
    ] {
        assert_eq!(
            identity(origin, Environment::Production).environment(),
            Environment::Production,
            "{origin:?} in a deployed bot is production traffic"
        );
        assert_eq!(
            identity(origin, Environment::Development).environment(),
            Environment::Development,
            "{origin:?} on a dev box must not report as production"
        );
    }
}

#[test]
fn an_in_game_actor_carries_a_player_name_not_a_snowflake() {
    let ingame = TurnIdentity {
        actor: TurnActor::Player("steve".to_owned()),
        channel: None,
        ..identity(TurnOrigin::Ingame, Environment::Production)
    };
    assert_eq!(
        ingame.actor,
        TurnActor::Player("steve".to_owned()),
        "in-game players have no Discord identity to attribute the turn to"
    );
    assert!(
        ingame.channel.is_none(),
        "an in-game reply is broadcast in world, not posted to a channel"
    );
}
