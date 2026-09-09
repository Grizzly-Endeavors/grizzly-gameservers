use super::*;

use crate::config::ThunderstoreMods;
use types::PackageId;

/// A minimal package zip carrying nothing but its manifest — enough for the
/// cache checks, which only read that.
fn package_zip(name: &str, version: &str) -> Vec<u8> {
    use std::io::Write as _;

    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(
            format!(r#"{{"name":"{name}","version_number":"{version}","dependencies":[]}}"#)
                .as_bytes(),
        )
        .unwrap();
    writer.finish().unwrap().into_inner()
}

fn mods(file: &str) -> ThunderstoreMods {
    ThunderstoreMods {
        file: file.to_owned(),
        community: "valheim".to_owned(),
        layout: archive::PackageLayout::BepInEx,
        provided: Vec::new(),
        work_dir: ".grizzly/thunderstore".to_owned(),
    }
}

#[test]
fn seeding_creates_the_list_and_never_truncates_an_authored_one() {
    // The seed runs before every launch, so truncating here would erase a mod
    // list on the next restart.
    let dir = tempfile::tempdir().unwrap();
    let path = seed_mods_file(dir.path(), ".grizzly/mods").unwrap();
    assert!(path.is_file());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");

    std::fs::write(&path, "ValheimModding-Jotunn\n").unwrap();
    seed_mods_file(dir.path(), ".grizzly/mods").unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "ValheimModding-Jotunn\n"
    );
}

#[test]
fn a_list_path_escaping_the_data_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    assert!(seed_mods_file(dir.path(), "../outside").is_err());
}

#[test]
fn the_resolved_file_lists_installs_and_explains_the_skips() {
    // Plain data the agent reads back to confirm an install, and the answer to
    // "why isn't the loader pack in here".
    let dir = tempfile::tempdir().unwrap();
    let installed = vec![
        "Author-Mod-1.0.0".parse::<PackageRelease>().unwrap(),
        "Lib-Core-2.0.0".parse().unwrap(),
    ];
    let provided: Vec<PackageId> = vec!["denikson-BepInExPack_Valheim".parse().unwrap()];
    write_resolved(dir.path(), &installed, &[], &provided).unwrap();

    let text = std::fs::read_to_string(dir.path().join(RESOLVED_FILE)).unwrap();
    assert!(text.contains("Author-Mod-1.0.0\n"), "{text}");
    assert!(text.contains("Lib-Core-2.0.0\n"), "{text}");
    assert!(
        text.contains("# denikson-BepInExPack_Valheim is supplied by the game image"),
        "{text}"
    );
}

#[test]
fn the_log_summary_names_packages_and_the_skipped_pack() {
    let logs = LogBuffer::new();
    let installed = vec!["Author-Mod-1.0.0".parse::<PackageRelease>().unwrap()];
    let provided: Vec<PackageId> = vec!["denikson-BepInExPack_Valheim".parse().unwrap()];
    report(&logs, &installed, &[], &provided, 3);

    let lines = logs.tail(10);
    let [summary, skipped] = lines.as_slice() else {
        panic!("expected a summary and a skip line, got {lines:?}")
    };
    assert!(summary.starts_with(LOG_PREFIX), "{summary}");
    assert!(summary.contains("Author-Mod-1.0.0"), "{summary}");
    assert!(skipped.contains("supplied by the game image"), "{skipped}");
}

#[test]
fn an_empty_list_still_says_so_rather_than_staying_silent() {
    // Silence after a restart reads as "the reconciler didn't run", which is a
    // different problem from "you haven't asked for any mods".
    let logs = LogBuffer::new();
    report(&logs, &[], &[], &[], 0);
    let lines = logs.tail(10);
    let [only] = lines.as_slice() else {
        panic!("expected exactly one line, got {lines:?}")
    };
    assert!(only.contains("no mods listed"), "{only}");
}

#[test]
fn resolved_records_the_client_only_half_of_the_modpack() {
    // The file is what an exported profile code is built from, so a package the
    // pack carries for players' machines has to survive into it — otherwise
    // every code minted off a running server drops exactly the entries a player
    // most needs.
    let dir = tempfile::tempdir().unwrap();
    let installed = vec!["Author-Mod-1.0.0".parse::<PackageRelease>().unwrap()];
    let client_only = vec![
        "RandyKnapp-EquipmentAndQuickSlots-3.0.2"
            .parse::<PackageRelease>()
            .unwrap(),
    ];
    write_resolved(dir.path(), &installed, &client_only, &[]).unwrap();

    let text = std::fs::read_to_string(dir.path().join(RESOLVED_FILE)).unwrap();
    let parsed = grizzly_control_api::mods::ResolvedMods::parse(&text);
    assert_eq!(
        parsed.installed,
        vec!["Author-Mod-1.0.0".to_owned()],
        "{text}"
    );
    assert_eq!(
        parsed.client_only,
        vec!["RandyKnapp-EquipmentAndQuickSlots-3.0.2".to_owned()],
        "{text}"
    );
}

#[test]
fn the_log_summary_names_the_packages_this_server_did_not_install() {
    let logs = LogBuffer::new();
    let installed = vec!["Author-Mod-1.0.0".parse::<PackageRelease>().unwrap()];
    let client_only = vec![
        "RandyKnapp-EquipmentAndQuickSlots-3.0.2"
            .parse::<PackageRelease>()
            .unwrap(),
    ];
    report(&logs, &installed, &client_only, &[], 3);

    let lines = logs.tail(10);
    let [_summary, client] = lines.as_slice() else {
        panic!("expected a summary and a client-only line, got {lines:?}")
    };
    assert!(client.contains("players' machines only"), "{client}");
    assert!(
        client.contains("RandyKnapp-EquipmentAndQuickSlots-3.0.2"),
        "{client}"
    );
}

#[tokio::test]
async fn a_game_that_did_not_opt_in_reconciles_nothing() {
    // Every image that sets none of the variables must behave exactly as it did
    // before this subsystem existed.
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::SupervisorConfig::from_env_with(&|_| None).unwrap();
    cfg.data_dir = dir.path().to_owned();
    cfg.thunderstore = None;
    let logs = LogBuffer::new();
    prepare(&cfg, &logs).await.unwrap();
    assert!(logs.tail(10).is_empty());
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn a_malformed_list_fails_the_launch_and_names_the_line() {
    // The failure the whole "report it as a crash" path exists for: the bot
    // restores the file's snapshot on the strength of this.
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::SupervisorConfig::from_env_with(&|_| None).unwrap();
    cfg.data_dir = dir.path().to_owned();
    cfg.thunderstore = Some(mods(".grizzly/mods"));
    std::fs::create_dir_all(dir.path().join(".grizzly")).unwrap();
    std::fs::write(dir.path().join(".grizzly/mods"), "not a mod at all\n").unwrap();

    let logs = LogBuffer::new();
    let err = prepare(&cfg, &logs).await.unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("line 1"), "{message}");
    assert!(message.contains(".grizzly/mods"), "{message}");
}

#[tokio::test]
async fn an_empty_list_reconciles_to_an_empty_staged_tree_with_no_network() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::SupervisorConfig::from_env_with(&|_| None).unwrap();
    cfg.data_dir = dir.path().to_owned();
    cfg.thunderstore = Some(mods(".grizzly/mods"));

    let logs = LogBuffer::new();
    prepare(&cfg, &logs).await.unwrap();

    assert!(dir.path().join(".grizzly/mods").is_file());
    assert!(
        dir.path()
            .join(".grizzly/thunderstore")
            .join(stage::STAGED_DIR)
            .is_dir()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join(".grizzly/thunderstore/resolved")).unwrap(),
        ""
    );
    assert!(
        logs.tail(10)
            .first()
            .is_some_and(|line| line.contains("no mods listed"))
    );
}

#[test]
fn a_missing_archive_is_not_a_cache_hit() {
    let dir = tempfile::tempdir().unwrap();
    let release: PackageRelease = "Author-Mod-1.0.0".parse().unwrap();
    assert!(!usable_in_cache(dir.path(), &release).unwrap());
}

#[test]
fn a_damaged_archive_is_not_a_cache_hit_either() {
    // Otherwise the download is skipped, staging fails on the damaged file, and
    // every restart afterwards fails identically — a permanent break with no way
    // out short of deleting the file by hand.
    let dir = tempfile::tempdir().unwrap();
    let release: PackageRelease = "Author-Mod-1.0.0".parse().unwrap();
    cache::store(dir.path(), &release, b"not a zip at all").unwrap();
    assert!(!usable_in_cache(dir.path(), &release).unwrap());
}

#[test]
fn an_archive_for_a_different_release_is_not_a_cache_hit() {
    // The manifest cross-check is what makes a cache hit trustworthy at all.
    let dir = tempfile::tempdir().unwrap();
    let wanted: PackageRelease = "Author-Mod-1.0.0".parse().unwrap();
    let other: PackageRelease = "Author-Mod-2.0.0".parse().unwrap();
    let bytes = package_zip("Mod", "2.0.0");
    cache::store(dir.path(), &wanted, &bytes).unwrap();

    assert!(!usable_in_cache(dir.path(), &wanted).unwrap());
    // Sanity: the same bytes *are* a hit for the release they actually describe.
    cache::store(dir.path(), &other, &bytes).unwrap();
    assert!(usable_in_cache(dir.path(), &other).unwrap());
}
