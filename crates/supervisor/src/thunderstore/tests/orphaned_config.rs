use super::*;

fn settings(guid: &str) -> String {
    format!(
        "## Settings file was created by plugin Thing v1.0.0\n\
         ## Plugin GUID: {guid}\n\
         \n\
         [General]\n\
         Enabled = true\n"
    )
}

fn stage_packages(root: &Path, packages: &[(&str, &str)]) {
    for (package, guid) in packages {
        let dir = root.join("BepInEx/plugins").join(package);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{package}.dll")),
            format!("MZ\0\0junk\0{guid}\0more junk"),
        )
        .unwrap();
    }
}

#[test]
fn a_header_names_the_owning_plugin() {
    assert_eq!(
        owning_guid(&settings("org.bepinex.plugins.mining")),
        Some("org.bepinex.plugins.mining")
    );
}

#[test]
fn a_file_with_no_header_belongs_to_nobody() {
    assert_eq!(owning_guid("[Caching]\n\n## Enable the cache\n"), None);
    assert_eq!(owning_guid(""), None);
    assert_eq!(owning_guid("## Plugin GUID:   \n"), None);
}

#[test]
fn a_settings_directory_is_named_after_its_guid() {
    assert_eq!(
        directory_guid("shudnal.ConditionalConfigSync"),
        Some("shudnal.ConditionalConfigSync")
    );
    assert_eq!(directory_guid("plugins"), None);
    assert_eq!(directory_guid(".hidden"), None);
}

#[test]
fn a_guid_is_found_anywhere_in_an_assembly() {
    assert!(contains(
        b"\0\0prefix-org.tristan.rcon-suffix",
        b"org.tristan.rcon"
    ));
    assert!(contains(b"exact", b"exact"));
    assert!(!contains(b"org.tristan.rco", b"org.tristan.rcon"));
    assert!(!contains(b"", b"anything"));
}

#[test]
fn settings_survive_while_their_mod_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(
        &staged,
        &[("Smoothbrain-Backpacks", "org.bepinex.plugins.backpacks")],
    );
    let kept = config.join("org.bepinex.plugins.backpacks.cfg");
    std::fs::write(&kept, settings("org.bepinex.plugins.backpacks")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report.removed, vec![]);
    assert_eq!(report.kept, 1);
    assert!(
        kept.exists(),
        "an installed mod's settings are never touched"
    );
}

#[test]
fn settings_of_an_uninstalled_mod_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(
        &staged,
        &[("Smoothbrain-Backpacks", "org.bepinex.plugins.backpacks")],
    );
    let orphan = config.join("org.bepinex.plugins.mining.cfg");
    std::fs::write(&orphan, settings("org.bepinex.plugins.mining")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(
        report.removed,
        vec![Orphan {
            entry: "org.bepinex.plugins.mining.cfg".to_owned(),
            guid: "org.bepinex.plugins.mining".to_owned(),
        }]
    );
    assert!(!orphan.exists());
}

#[test]
fn a_settings_directory_goes_as_a_unit() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    let orphan = config.join("jg224.chestflow");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("Feeding.cfg"), settings("jg224.chestflow")).unwrap();
    let live = config.join("shudnal.ExtraSlots");
    std::fs::create_dir_all(&live).unwrap();
    std::fs::write(live.join("Slots.cfg"), settings("shudnal.ExtraSlots")).unwrap();
    stage_packages(&staged, &[("shudnal-ExtraSlots", "shudnal.ExtraSlots")]);

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(
        report.removed,
        vec![Orphan {
            entry: "jg224.chestflow".to_owned(),
            guid: "jg224.chestflow".to_owned(),
        }]
    );
    assert!(!orphan.exists());
    assert!(live.join("Slots.cfg").exists());
}

#[test]
fn the_loaders_own_configuration_is_not_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(&staged, &[("ValheimModding-Jotunn", "com.jotunn.jotunn")]);
    for name in ["BepInEx.cfg", "BepInEx.cfg.default"] {
        std::fs::write(config.join(name), "[Caching]\nEnabled = true\n").unwrap();
    }

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report, SweepReport::default());
    assert!(config.join("BepInEx.cfg").exists());
}

#[test]
fn a_snapshot_left_by_the_file_tools_is_not_a_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(
        &staged,
        &[("Zellds-SmartCraftStorage", "com.zellds.smartcraft")],
    );
    let backup = config.join("jg224.chestflow.cfg.grizzly.bak");
    std::fs::write(&backup, settings("jg224.chestflow")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report, SweepReport::default());
    assert!(backup.exists());
}

#[test]
fn settings_the_image_owns_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(&staged, &[("ValheimModding-Jotunn", "com.jotunn.jotunn")]);
    let owned = config.join("org.tristan.rcon.cfg");
    std::fs::write(&owned, settings("org.tristan.rcon")).unwrap();

    let report = sweep(&config, &staged, &["org.tristan.rcon.cfg".to_owned()]).unwrap();

    assert_eq!(report, SweepReport::default());
    assert!(owned.exists(), "the image's own settings are never swept");
}

#[test]
fn an_absent_staged_tree_sweeps_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    std::fs::create_dir_all(&config).unwrap();
    let untouched = config.join("org.bepinex.plugins.mining.cfg");
    std::fs::write(&untouched, settings("org.bepinex.plugins.mining")).unwrap();

    let report = sweep(&config, &dir.path().join("never-staged"), &[]).unwrap();

    assert_eq!(report, SweepReport::default());
    assert!(untouched.exists());
}

#[test]
fn an_empty_staged_tree_orphans_everything() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(config.join("../staged/BepInEx/plugins")).unwrap();
    std::fs::create_dir_all(&config).unwrap();
    let orphan = config.join("org.bepinex.plugins.mining.cfg");
    std::fs::write(&orphan, settings("org.bepinex.plugins.mining")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report.removed.len(), 1);
    assert!(!orphan.exists());
}

#[test]
fn a_missing_config_directory_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("staged");
    stage_packages(&staged, &[("ValheimModding-Jotunn", "com.jotunn.jotunn")]);

    let report = sweep(&dir.path().join("no-such-dir"), &staged, &[]).unwrap();

    assert_eq!(report, SweepReport::default());
}

#[test]
fn a_guid_claimed_by_any_assembly_is_live() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    let plugins = staged.join("BepInEx/plugins/shudnal-ExtraSlots");
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(
        plugins.join("ExtraSlots.dll"),
        "MZ\0shudnal.ExtraSlots\0shudnal.ConditionalConfigSync\0",
    )
    .unwrap();
    let borrowed = config.join("shudnal.ConditionalConfigSync.cfg");
    std::fs::write(&borrowed, settings("shudnal.ConditionalConfigSync")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report.kept, 1);
    assert!(borrowed.exists());
}

#[test]
fn a_header_beyond_the_read_window_leaves_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("bepinex");
    let staged = dir.path().join("staged");
    std::fs::create_dir_all(&config).unwrap();
    stage_packages(&staged, &[("ValheimModding-Jotunn", "com.jotunn.jotunn")]);
    let buried = config.join("deep.cfg");
    let padding = "# ".repeat(HEADER_BYTES);
    std::fs::write(&buried, format!("{padding}\n## Plugin GUID: deep.plugin\n")).unwrap();

    let report = sweep(&config, &staged, &[]).unwrap();

    assert_eq!(report, SweepReport::default());
    assert!(buried.exists());
}
