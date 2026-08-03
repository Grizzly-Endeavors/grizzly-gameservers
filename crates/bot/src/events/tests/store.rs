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
async fn a_disabled_log_accepts_and_discards_resource_samples() {
    let log = EventLog::connect(None).await;

    // The resource cycle writes unconditionally, exactly like the occupancy
    // cycle — a disabled log must absorb the tick as a no-op.
    log.record_resource_samples(&[ResourceSample {
        instance: crate::domain::InstanceName::new("worldly-otter"),
        game: None,
        guild: None,
        cpu_millicores: Some(120),
        mem_bytes: Some(1_610_612_736),
        cpu_request_millicores: Some(530),
        mem_request_bytes: Some(2_231_369_728),
        cpu_limit_millicores: Some(2030),
        mem_limit_bytes: Some(4_378_853_376),
        pvc_bytes: Some(5_368_709_120),
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
