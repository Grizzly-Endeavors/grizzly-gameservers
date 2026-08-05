use super::*;

#[test]
fn backup_keys_nest_under_guild_and_instance() {
    let keys = backup_keys("998877", "nether", "20260707T143000Z");
    assert_eq!(
        keys.tarball,
        "backups/998877/nether/20260707T143000Z.tar.zst"
    );
    assert_eq!(
        keys.manifest,
        "backups/998877/nether/20260707T143000Z.manifest.json"
    );
}

#[test]
fn two_guilds_reusing_one_instance_name_get_separate_prefixes() {
    // The whole point of the layout: `/destroy` frees a friend-chosen name for the
    // next guild to claim, and the previous owner's backups outlive the instance.
    assert_ne!(backup_prefix("111", "smp"), backup_prefix("222", "smp"));
}

#[test]
fn an_absent_guild_takes_the_reserved_segment_rather_than_collapsing() {
    // An empty segment would render `backups//smp/`, which S3 normalises back into
    // the shared namespace this layout exists to avoid.
    assert_eq!(
        backup_prefix("", "smp"),
        format!("backups/{UNSCOPED_GUILD_SEGMENT}/smp/")
    );
    assert_eq!(
        archive_prefix("", "smp"),
        format!("archives/{UNSCOPED_GUILD_SEGMENT}/smp/")
    );
}

#[test]
fn the_reserved_segment_cannot_collide_with_a_discord_guild_id() {
    assert!(
        !UNSCOPED_GUILD_SEGMENT.chars().all(|c| c.is_ascii_digit()),
        "a snowflake is all digits, so the reserved segment must not be"
    );
}

#[test]
fn archive_keys_nest_under_guild_and_name() {
    let keys = archive_keys("998877", "nether", "20260707T143000Z");
    assert_eq!(
        keys.tarball,
        "archives/998877/nether/20260707T143000Z.tar.zst"
    );
    assert_eq!(
        keys.manifest,
        "archives/998877/nether/20260707T143000Z.manifest.json"
    );
}

#[test]
fn manifest_key_for_derives_the_sidecar_from_the_tarball() {
    assert_eq!(
        manifest_key_for("backups/nether/20260707T143000Z.tar.zst").as_deref(),
        Some("backups/nether/20260707T143000Z.manifest.json")
    );
    assert_eq!(manifest_key_for("not-a-tarball.txt"), None);
}

#[test]
fn keys_to_prune_keeps_the_newest_and_returns_the_rest_oldest_first() {
    // Deliberately unsorted; lexicographic == chronological for the stamp format.
    let keys = vec![
        "backups/x/20260707T120000Z.tar.zst".to_owned(),
        "backups/x/20260707T150000Z.tar.zst".to_owned(),
        "backups/x/20260707T090000Z.tar.zst".to_owned(),
        "backups/x/20260707T180000Z.tar.zst".to_owned(),
    ];
    let prune = keys_to_prune(keys, 2);
    assert_eq!(
        prune,
        vec![
            "backups/x/20260707T090000Z.tar.zst".to_owned(),
            "backups/x/20260707T120000Z.tar.zst".to_owned(),
        ],
        "the two oldest of four should be pruned to keep 2"
    );
}

#[test]
fn keys_to_prune_keeps_everything_when_under_the_limit() {
    let keys = vec![
        "backups/x/20260707T120000Z.tar.zst".to_owned(),
        "backups/x/20260707T150000Z.tar.zst".to_owned(),
    ];
    assert!(
        keys_to_prune(keys, 7).is_empty(),
        "nothing prunes when fewer than keep-N exist"
    );
}

#[test]
fn keys_to_prune_with_zero_keep_returns_every_key() {
    let keys = vec![
        "backups/x/20260707T120000Z.tar.zst".to_owned(),
        "backups/x/20260707T150000Z.tar.zst".to_owned(),
        "backups/x/20260707T090000Z.tar.zst".to_owned(),
    ];
    // keep == 0 deletes everything — the documented contract callers must reject
    // upstream. Pinned so a future "helpful" floor-guard can't silently soften it.
    assert_eq!(
        keys_to_prune(keys.clone(), 0).len(),
        keys.len(),
        "zero retention returns every key"
    );
}

#[test]
fn manifest_round_trips_through_json() {
    let manifest = BackupManifest {
        schema: MANIFEST_SCHEMA,
        kind: ArtifactKind::Archive,
        instance: InstanceName::new("nether"),
        game: GameId::new("minecraft"),
        guild: GuildId::new("998877"),
        created_by: "12345".to_owned(),
        created_at: "2026-07-07T14:30:00Z".to_owned(),
        tarball_key: "archives/998877/nether/20260707T143000Z.tar.zst".to_owned(),
        size_bytes: 4096,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let parsed: BackupManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, manifest);
}
