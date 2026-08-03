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
async fn a_disabled_log_never_reports_a_guild_as_newly_joined() {
    let log = EventLog::connect(None).await;
    assert!(!log.claim_guild(42).await);
    log.seed_known_guilds(&[42]).await;
    assert!(!log.claim_guild(42).await);
}
