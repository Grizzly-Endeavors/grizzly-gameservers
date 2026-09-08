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
    // The whole reason the tree is rebuilt rather than reconciled: deleting a
    // line has to delete the files, without tracking what each package wrote.
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
fn an_empty_mod_list_leaves_an_empty_tree_rather_than_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let one = release("Author-One-1.0.0");
    cache::store(dir.path(), &one, &package(&one, &[("One.dll", b"MZ")])).unwrap();
    rebuild(dir.path(), &[resolved(&one)], PackageLayout::BepInEx).unwrap();

    rebuild(dir.path(), &[], PackageLayout::BepInEx).unwrap();
    assert!(staged_path(dir.path()).is_dir());
    assert!(!staged_path(dir.path()).join("BepInEx").exists());
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
    // The swap is what buys this: a half-built tree is never what the image
    // mirrors, because it is never the tree the image looks at.
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
