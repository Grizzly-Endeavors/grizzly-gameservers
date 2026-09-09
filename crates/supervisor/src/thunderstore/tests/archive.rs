use std::io::{Cursor, Write};
use std::path::Path;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;

fn id(text: &str) -> PackageId {
    text.parse().expect("fixture id parses")
}

fn release(text: &str) -> PackageRelease {
    text.parse().expect("fixture release parses")
}

fn plan(path: &str) -> Option<String> {
    plan_layout(PackageLayout::BepInEx, &id("Author-Mod"), Path::new(path))
        .map(|planned| planned.to_string_lossy().replace('\\', "/"))
}

/// Build a package zip in memory. Stored rather than deflated so the fixtures
/// stay readable as bytes and no compressor is exercised by accident.
fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, body) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn manifest(name: &str, version: &str) -> Vec<u8> {
    format!(r#"{{"name":"{name}","version_number":"{version}","dependencies":[]}}"#).into_bytes()
}

// ---- layout ----

#[test]
fn a_plugins_directory_gets_a_per_package_subdirectory() {
    // Per-package so removing a mod is removing one directory, which is what
    // makes "delete the line" actually delete the files.
    assert_eq!(
        plan("plugins/Jotunn.dll").as_deref(),
        Some("BepInEx/plugins/Author-Mod/Jotunn.dll")
    );
}

#[test]
fn a_leading_bepinex_directory_is_stripped_first() {
    // The two common package shapes are the same package.
    assert_eq!(
        plan("BepInEx/plugins/Jotunn.dll"),
        plan("plugins/Jotunn.dll")
    );
}

#[test]
fn patchers_and_monomod_follow_the_same_rule() {
    assert_eq!(
        plan("patchers/Patch.dll").as_deref(),
        Some("BepInEx/patchers/Author-Mod/Patch.dll")
    );
    assert_eq!(
        plan("BepInEx/monomod/Hook.mm.dll").as_deref(),
        Some("BepInEx/monomod/Author-Mod/Hook.mm.dll")
    );
}

#[test]
fn config_lands_flat_because_plugins_look_it_up_by_filename() {
    assert_eq!(
        plan("config/author.mod.cfg").as_deref(),
        Some("BepInEx/config/author.mod.cfg")
    );
}

#[test]
fn a_bare_dll_at_the_root_becomes_a_plugin() {
    // Smoothbrain's packages ship exactly this way.
    assert_eq!(
        plan("Sailing.dll").as_deref(),
        Some("BepInEx/plugins/Author-Mod/Sailing.dll")
    );
}

#[test]
fn an_unrecognised_folder_is_kept_whole_under_the_package() {
    // Asset folders are addressed relative to the plugin, so the tree has to
    // survive intact rather than being flattened.
    assert_eq!(
        plan("assets/textures/thing.png").as_deref(),
        Some("BepInEx/plugins/Author-Mod/assets/textures/thing.png")
    );
}

#[test]
fn the_loader_core_is_never_overwritten() {
    // Only the image's own provided pack may write here; anything else doing so
    // breaks the loader that everything else depends on.
    assert_eq!(plan("core/BepInEx.Preloader.dll"), None);
    assert_eq!(plan("BepInEx/core/BepInEx.Preloader.dll"), None);
}

#[test]
fn package_metadata_is_not_installed() {
    for name in ["manifest.json", "README.md", "CHANGELOG.md", "icon.png"] {
        assert_eq!(plan(name), None, "{name} should not be installed");
    }
}

#[test]
fn metadata_names_deeper_in_the_tree_are_still_installed() {
    // Only the *root* files are metadata; a mod shipping its own readme inside
    // an asset folder is shipping a file it expects to find there.
    assert_eq!(
        plan("assets/README.md").as_deref(),
        Some("BepInEx/plugins/Author-Mod/assets/README.md")
    );
}

#[test]
fn directory_names_match_case_insensitively() {
    // Packages spell these both ways and neither is wrong upstream.
    assert_eq!(
        plan("Plugins/Thing.dll").as_deref(),
        Some("BepInEx/plugins/Author-Mod/Thing.dll")
    );
    assert_eq!(plan("bepinex/CORE/loader.dll"), None);
}

#[test]
fn a_directory_entry_with_nothing_under_it_addresses_nothing() {
    assert_eq!(plan("plugins"), None);
    assert_eq!(plan("BepInEx"), None);
}

#[test]
fn the_layout_name_is_a_closed_vocabulary() {
    assert_eq!(
        PackageLayout::parse("bepinex"),
        Some(PackageLayout::BepInEx)
    );
    assert_eq!(
        PackageLayout::parse(" BepInEx "),
        Some(PackageLayout::BepInEx)
    );
    assert_eq!(PackageLayout::parse("ue4ss"), None);
}

// ---- manifest cross-check ----

#[test]
fn a_manifest_matching_the_filename_is_accepted() {
    let bytes = zip_with(&[("manifest.json", &manifest("Mod", "1.0.0"))]);
    let found = read_manifest(Cursor::new(bytes), &release("Author-Mod-1.0.0")).unwrap();
    assert_eq!(found.name, "Mod");
}

#[test]
fn a_manifest_with_a_utf8_byte_order_mark_is_accepted() {
    // Thunderstore packages are authored on Windows more often than not, and a
    // good share ship a manifest starting with EF BB BF. JSON has no notion of
    // a BOM, so refusing one means refusing packages r2modman installs fine —
    // and the refusal lands as a launch failure, not as a skipped mod.
    let mut with_bom = b"\xef\xbb\xbf".to_vec();
    with_bom.extend_from_slice(&manifest("Mod", "1.0.0"));
    let bytes = zip_with(&[("manifest.json", &with_bom)]);
    let found = read_manifest(Cursor::new(bytes), &release("Author-Mod-1.0.0")).unwrap();
    assert_eq!(found.name, "Mod");
}

#[test]
fn a_zip_holding_a_different_package_is_refused() {
    // A swapped or mislabeled cache file would otherwise install under the name
    // the mod list asked for.
    let bytes = zip_with(&[("manifest.json", &manifest("Other", "1.0.0"))]);
    let err = read_manifest(Cursor::new(bytes), &release("Author-Mod-1.0.0")).unwrap_err();
    assert!(format!("{err:#}").contains("Other"), "{err:#}");
}

#[test]
fn a_zip_holding_a_different_version_is_refused() {
    let bytes = zip_with(&[("manifest.json", &manifest("Mod", "2.0.0"))]);
    let err = read_manifest(Cursor::new(bytes), &release("Author-Mod-1.0.0")).unwrap_err();
    assert!(format!("{err:#}").contains("2.0.0"), "{err:#}");
}

#[test]
fn a_zip_with_no_manifest_is_refused() {
    let bytes = zip_with(&[("plugins/Thing.dll", b"MZ")]);
    let err = read_manifest(Cursor::new(bytes), &release("Author-Mod-1.0.0")).unwrap_err();
    assert!(format!("{err:#}").contains("manifest.json"), "{err:#}");
}

// ---- extraction ----

fn extract_to(bytes: Vec<u8>, dest: &Path) -> anyhow::Result<usize> {
    extract(
        Cursor::new(bytes),
        &release("Author-Mod-1.0.0"),
        PackageLayout::BepInEx,
        dest,
    )
}

#[test]
fn extraction_lays_a_package_out_and_drops_its_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("README.md", b"docs"),
        ("plugins/Thing.dll", b"MZ"),
        ("config/thing.cfg", b"key=value"),
    ]);
    let installed = extract_to(bytes, dir.path()).unwrap();
    assert_eq!(installed, 2);
    assert!(
        dir.path()
            .join("BepInEx/plugins/Author-Mod/Thing.dll")
            .is_file()
    );
    assert!(dir.path().join("BepInEx/config/thing.cfg").is_file());
    assert!(
        !dir.path()
            .join("BepInEx/plugins/Author-Mod/README.md")
            .exists()
    );
}

#[test]
fn an_entry_climbing_out_of_the_archive_is_refused() {
    // Refused whole rather than sanitised: an archive doing this has no reason
    // to, and its other files are no more trustworthy.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("../evil.dll", b"MZ"),
    ]);
    let err = extract_to(bytes, dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("climbs out"), "{err:#}");
}

#[test]
fn an_absolute_entry_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("/etc/passwd", b"root"),
    ]);
    let err = extract_to(bytes, dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("absolute"), "{err:#}");
}

#[test]
fn a_backslash_entry_that_climbs_is_still_refused() {
    // Backslashes are separators, so this is `../evil.dll` and has to be caught
    // as the traversal it is. Normalising first is what makes that possible: on
    // Unix `Path::new("..\\evil.dll")` is one component literally named
    // `..\evil.dll`, which `ParentDir` would never match.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        (r"..\evil.dll", b"MZ"),
    ]);
    let err = extract_to(bytes, dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("climbs out"), "{err:#}");
}

#[test]
fn a_windows_packed_entry_installs_rather_than_being_refused() {
    // A real package hit this: ArgusMagnus-ServersideQoL-2.0.0 is zipped with
    // backslash separators, which is a fact about the packer rather than a
    // security problem — r2modman installs it fine, and so must this.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        (r"plugins\Mod.dll", b"MZ"),
    ]);
    extract_to(bytes, dir.path()).expect("a windows-packed archive installs");
    let installed = dir.path().join("BepInEx/plugins/Author-Mod/Mod.dll");
    assert!(installed.is_file(), "expected {}", installed.display());
}

#[test]
fn the_real_serverside_qol_shape_routes_to_both_loader_dirs() {
    // ArgusMagnus-ServersideQoL-2.0.0 as actually published: a mixed archive
    // whose metadata uses no separator and whose nested entries use backslashes,
    // spanning two per-package loader directories. This is the package that
    // exposed the defect, so its exact shape is the regression test.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("README.md", b"readme"),
        (r"patchers\Mod.Patchers.dll", b"MZ"),
        (r"plugins\Mod.deps.json", b"{}"),
        (r"plugins\Mod.dll", b"MZ"),
    ]);
    extract_to(bytes, dir.path()).expect("the published archive installs");
    for expected in [
        "BepInEx/patchers/Author-Mod/Mod.Patchers.dll",
        "BepInEx/plugins/Author-Mod/Mod.deps.json",
        "BepInEx/plugins/Author-Mod/Mod.dll",
    ] {
        let path = dir.path().join(expected);
        assert!(path.is_file(), "expected {}", path.display());
    }
}

#[test]
fn a_backslash_never_becomes_part_of_a_filename() {
    // Planning the layout on the raw name would treat `a\b.dll` as one filename
    // and write a file with a backslash in it, which nothing would ever load.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        (r"BepInEx\plugins\nested\Deep.dll", b"MZ"),
    ]);
    extract_to(bytes, dir.path()).expect("a nested windows-packed archive installs");
    let installed = dir
        .path()
        .join("BepInEx/plugins/Author-Mod/nested/Deep.dll");
    assert!(installed.is_file(), "expected {}", installed.display());
}

#[test]
fn a_symlink_entry_is_refused() {
    // A symlink is the other way out of the tree, and it survives the name check.
    let dir = tempfile::tempdir().unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("manifest.json", options).unwrap();
    writer.write_all(&manifest("Mod", "1.0.0")).unwrap();
    writer
        .add_symlink("plugins/escape.dll", "/etc/passwd", options)
        .unwrap();
    let bytes = writer.finish().unwrap().into_inner();
    let err = extract_to(bytes, dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("symlink"), "{err:#}");
}

#[test]
fn a_package_with_too_many_files_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("manifest.json", options).unwrap();
    writer.write_all(&manifest("Mod", "1.0.0")).unwrap();
    for index in 0..=MAX_ENTRIES {
        writer
            .start_file(format!("plugins/f{index}.dll"), options)
            .unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    let err = extract_to(bytes, dir.path()).unwrap_err();
    assert!(format!("{err:#}").contains("over the"), "{err:#}");
}

#[test]
fn the_loader_core_is_skipped_during_extraction_rather_than_failing() {
    // A package that ships a core/ directory is not malicious, it is just wrong
    // about what it owns — install the rest of it.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("BepInEx/core/BepInEx.Preloader.dll", b"MZ"),
        ("BepInEx/plugins/Thing.dll", b"MZ"),
    ]);
    let installed = extract_to(bytes, dir.path()).unwrap();
    assert_eq!(installed, 1);
    assert!(!dir.path().join("BepInEx/core").exists());
}

#[test]
fn an_oversized_entry_is_refused_before_it_lands() {
    // Exercised against small caps rather than the production 256 MiB, which no
    // fixture can honestly reach — the guard is the same code either way.
    let dir = tempfile::tempdir().unwrap();
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("plugins/Huge.dll", &vec![0_u8; 4096]),
    ]);
    let caps = Caps {
        entry_bytes: 1024,
        ..Caps::DEFAULT
    };
    let err = extract_within(
        Cursor::new(bytes),
        &release("Author-Mod-1.0.0"),
        PackageLayout::BepInEx,
        dir.path(),
        caps,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("larger than 1024"), "{err:#}");
}

#[test]
fn an_archive_that_expands_past_the_total_is_refused_part_way() {
    // The zip-bomb guard: no single entry is over the per-entry cap, but their
    // sum is, and the check has to fire while streaming rather than after the
    // whole thing has landed on disk.
    let dir = tempfile::tempdir().unwrap();
    let chunk = vec![0_u8; 1024];
    let bytes = zip_with(&[
        ("manifest.json", &manifest("Mod", "1.0.0")),
        ("plugins/One.dll", &chunk),
        ("plugins/Two.dll", &chunk),
        ("plugins/Three.dll", &chunk),
    ]);
    let caps = Caps {
        entry_bytes: 2048,
        total_bytes: 2048,
        ..Caps::DEFAULT
    };
    let err = extract_within(
        Cursor::new(bytes),
        &release("Author-Mod-1.0.0"),
        PackageLayout::BepInEx,
        dir.path(),
        caps,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("expands past 2048"), "{err:#}");
}

#[test]
fn the_production_caps_are_the_ones_a_real_launch_enforces() {
    // The parameterised form exists for the tests above; a drift between it and
    // the consts would make those tests prove nothing about production.
    assert_eq!(
        Caps::DEFAULT,
        Caps {
            entries: MAX_ENTRIES,
            entry_bytes: MAX_ENTRY_BYTES,
            total_bytes: MAX_TOTAL_BYTES,
        }
    );
}
