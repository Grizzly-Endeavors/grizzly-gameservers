use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::*;
use crate::thunderstore::cache;
use crate::thunderstore::types::PackageRelease;

fn release(text: &str) -> PackageRelease {
    text.parse().expect("fixture release parses")
}

fn package(release: &PackageRelease, files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    writer.start_file("manifest.json", options).unwrap();
    writer
        .write_all(
            format!(
                r#"{{"name":"{}","version_number":"{}","dependencies":[]}}"#,
                release.id.name, release.version
            )
            .as_bytes(),
        )
        .unwrap();
    for (name, body) in files {
        writer.start_file(*name, options).unwrap();
        writer.write_all(body).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn resolved(release: &PackageRelease) -> ResolvedPackage {
    ResolvedPackage {
        release: release.clone(),
        pinned: false,
    }
}

#[test]
fn the_staged_tree_holds_exactly_what_was_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let one = release("Author-One-1.0.0");
    let two = release("Author-Two-1.0.0");
    cache::store(
        dir.path(),
        &one,
        &package(&one, &[("plugins/One.dll", b"MZ")]),
    )
    .unwrap();
    cache::store(dir.path(), &two, &package(&two, &[("Two.dll", b"MZ")])).unwrap();

    let installed = rebuild(
        dir.path(),
        &[resolved(&one), resolved(&two)],
        PackageLayout::BepInEx,
    )
    .unwrap();
    assert_eq!(installed, 2);
    let staged = staged_path(dir.path());
    assert!(staged.join("BepInEx/plugins/Author-One/One.dll").is_file());
    assert!(staged.join("BepInEx/plugins/Author-Two/Two.dll").is_file());
}

#[test]
fn a_removed_package_is_gone_after_the_next_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let one = release("Author-One-1.0.0");
    let two = release("Author-Two-1.0.0");
    cache::store(dir.path(), &one, &package(&one, &[("One.dll", b"MZ")])).unwrap();
    cache::store(dir.path(), &two, &package(&two, &[("Two.dll", b"MZ")])).unwrap();

    rebuild(
        dir.path(),
        &[resolved(&one), resolved(&two)],
        PackageLayout::BepInEx,
    )
    .unwrap();
    rebuild(dir.path(), &[resolved(&one)], PackageLayout::BepInEx).unwrap();

    let staged = staged_path(dir.path());
    assert!(staged.join("BepInEx/plugins/Author-One/One.dll").is_file());
    assert!(!staged.join("BepInEx/plugins/Author-Two").exists());
}

#[test]
fn an_empty_mod_list_stages_an_empty_tree_not_an_absent_one() {
    let dir = tempfile::tempdir().unwrap();
    let one = release("Author-One-1.0.0");
    cache::store(dir.path(), &one, &package(&one, &[("One.dll", b"MZ")])).unwrap();
    rebuild(dir.path(), &[resolved(&one)], PackageLayout::BepInEx).unwrap();

    rebuild(dir.path(), &[], PackageLayout::BepInEx).unwrap();
    let staged = staged_path(dir.path());
    assert!(
        staged.join("BepInEx/plugins").is_dir(),
        "the skeleton stays"
    );
    assert!(
        !staged.join("BepInEx/plugins/Author-One").exists(),
        "but the package is gone"
    );
}

#[test]
fn every_skeleton_directory_is_present_even_with_no_mods() {
    let dir = tempfile::tempdir().unwrap();
    rebuild(dir.path(), &[], PackageLayout::BepInEx).unwrap();
    for relative in PackageLayout::BepInEx.skeleton() {
        assert!(
            staged_path(dir.path()).join(relative).is_dir(),
            "{relative} must exist so the mirror can clear it"
        );
    }
}

#[test]
fn the_skeleton_never_includes_the_loader_core() {
    assert!(
        !PackageLayout::BepInEx
            .skeleton()
            .iter()
            .any(|dir| dir.ends_with("core")),
    );
}

#[test]
fn a_package_missing_from_the_cache_fails_the_rebuild_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let err = rebuild(
        dir.path(),
        &[resolved(&release("Author-Gone-1.0.0"))],
        PackageLayout::BepInEx,
    )
    .unwrap_err();
    assert!(format!("{err:#}").contains("Author-Gone-1.0.0"), "{err:#}");
}

#[test]
fn a_bad_archive_leaves_the_previous_tree_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let good = release("Author-Good-1.0.0");
    cache::store(dir.path(), &good, &package(&good, &[("Good.dll", b"MZ")])).unwrap();
    rebuild(dir.path(), &[resolved(&good)], PackageLayout::BepInEx).unwrap();

    let bad = release("Author-Bad-1.0.0");
    cache::store(
        dir.path(),
        &bad,
        &package(&bad, &[("../escape.dll", b"MZ")]),
    )
    .unwrap();
    assert!(
        rebuild(
            dir.path(),
            &[resolved(&good), resolved(&bad)],
            PackageLayout::BepInEx
        )
        .is_err()
    );
    assert!(
        staged_path(dir.path())
            .join("BepInEx/plugins/Author-Good/Good.dll")
            .is_file(),
        "the last good tree must survive a failed rebuild"
    );
}
