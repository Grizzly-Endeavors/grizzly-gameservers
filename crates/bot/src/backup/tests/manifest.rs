use super::*;

#[test]
fn backup_keys_pair_tarball_and_manifest_under_the_instance_prefix() {
    let keys = backup_keys("nether", "20260707T143000Z");
    assert_eq!(keys.tarball, "backups/nether/20260707T143000Z.tar.zst");
    assert_eq!(
        keys.manifest,
        "backups/nether/20260707T143000Z.manifest.json"
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
fn manifest_round_trips_through_json() {
    let manifest = BackupManifest {
        schema: MANIFEST_SCHEMA,
        kind: ArtifactKind::Archive,
        instance: "nether".to_owned(),
        game: "minecraft".to_owned(),
        guild: "998877".to_owned(),
        created_by: "12345".to_owned(),
        created_at: "2026-07-07T14:30:00Z".to_owned(),
        tarball_key: "archives/998877/nether/20260707T143000Z.tar.zst".to_owned(),
        size_bytes: 4096,
    };
    let json = serde_json::to_string(&manifest).unwrap();
    let parsed: BackupManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, manifest);
}
