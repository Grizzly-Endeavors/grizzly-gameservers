use super::*;

use crate::events::catalog::{Actor, Attribution, EventKind, Surface};

#[tokio::test]
async fn a_disabled_log_accepts_and_discards() {
    let log = EventLog::connect(None).await;
    assert!(!log.enabled());

    // The contract that lets emission sites stay unconditional: recording into a
    // disabled log is a no-op, not an error and not a panic.
    log.record(ProductEvent::new(
        EventKind::ServerDestroyed,
        &Attribution::new(Actor::Discord(1), Surface::Command),
        "destroyed",
    ))
    .await;
}

#[tokio::test]
async fn a_disabled_log_accepts_and_discards_samples() {
    let log = EventLog::connect(None).await;

    // Same contract on the sampler's path: a tick with nowhere to land is a
    // no-op, so the sampler never has to check before writing.
    log.record_samples(&[OccupancySample {
        instance: crate::domain::InstanceName::new("worldly-otter"),
        game: None,
        guild: None,
        players: Some(2),
        restarts: 0,
    }])
    .await;
}

#[tokio::test]
async fn a_disabled_log_never_reports_a_guild_as_newly_joined() {
    let log = EventLog::connect(None).await;
    assert!(!log.claim_guild(42).await);
    log.seed_known_guilds(&[42]).await;
    assert!(!log.claim_guild(42).await);
}
