use super::*;

fn target() -> LiveInstance {
    LiveInstance {
        instance: InstanceName::new("minecraft-7sar1"),
        game: GameId::new("minecraft"),
        guild: GuildId::new("989178118180847666"),
        running: true,
        unhealthy: false,
    }
}

#[test]
fn scheduled_snapshot_event_carries_the_instance_it_backed_up() {
    let event = BackupService::snapshot_event(
        EventKind::BackupTaken,
        &Attribution::system(),
        &target(),
        &BackupOutcome::BackedUp { size_bytes: 4096 },
    );
    assert_eq!(event.kind, EventKind::BackupTaken);
    assert_eq!(
        event.instance.as_ref().map(InstanceName::as_str),
        Some("minecraft-7sar1"),
        "a scheduled backup must be attributable to its server"
    );
    assert_eq!(
        event.guild.as_ref().map(GuildId::as_str),
        Some("989178118180847666"),
        "the owning guild comes off the target's labels"
    );
}

#[test]
fn a_failed_scheduled_snapshot_is_still_recorded_against_its_instance() {
    let event = BackupService::snapshot_event(
        EventKind::BackupTaken,
        &Attribution::system(),
        &target(),
        &BackupOutcome::NotRunning,
    );
    assert_eq!(
        event.instance.as_ref().map(InstanceName::as_str),
        Some("minecraft-7sar1"),
        "a failed backup is still attributable to its server"
    );
    assert_ne!(
        event.outcome,
        mapping::backup(&BackupOutcome::BackedUp { size_bytes: 1 }),
        "a failure must not record the same outcome as a success"
    );
}
