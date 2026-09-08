use super::*;

fn release(text: &str) -> PackageRelease {
    text.parse().expect("fixture release parses")
}

#[test]
fn a_cache_path_is_the_release_string() {
    // The filename *is* the index — there is no separate manifest to fall out of
    // sync with what's on disk.
    let dir = std::path::Path::new("/data/.grizzly/thunderstore");
    assert_eq!(
        path_for(dir, &release("Author-Mod-1.2.3")),
        dir.join("cache/Author-Mod-1.2.3.zip")
    );
}

#[test]
fn a_stored_archive_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let release = release("Author-Mod-1.2.3");
    assert!(open(dir.path(), &release).unwrap().is_none());
    store(dir.path(), &release, b"zip bytes").unwrap();
    assert!(open(dir.path(), &release).unwrap().is_some());
    assert_eq!(cached_releases(dir.path()), vec![release]);
}

#[test]
fn a_partial_download_is_never_a_cache_hit() {
    // An interrupted download must not look like a complete package.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(CACHE_DIR)).unwrap();
    std::fs::write(
        dir.path().join(CACHE_DIR).join("Author-Mod-1.2.3.zip.part"),
        b"half",
    )
    .unwrap();
    assert!(
        open(dir.path(), &release("Author-Mod-1.2.3"))
            .unwrap()
            .is_none()
    );
    assert!(cached_releases(dir.path()).is_empty());
}

#[test]
fn an_unparseable_filename_is_ignored_rather_than_fatal() {
    // The cache is derived state; a stray file in it must not stop a launch.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(CACHE_DIR)).unwrap();
    std::fs::write(dir.path().join(CACHE_DIR).join("notes.txt"), b"hi").unwrap();
    assert!(cached_releases(dir.path()).is_empty());
}

#[test]
fn collection_keeps_the_resolved_set_and_removes_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let keep = release("Author-Keep-1.0.0");
    let drop = release("Author-Drop-1.0.0");
    store(dir.path(), &keep, b"a").unwrap();
    store(dir.path(), &drop, b"b").unwrap();
    std::fs::write(
        dir.path().join(CACHE_DIR).join("leftover.zip.part"),
        b"half",
    )
    .unwrap();

    let removed = collect(dir.path(), std::slice::from_ref(&keep)).unwrap();
    assert_eq!(removed, 2, "the dropped package and the partial both go");
    assert_eq!(cached_releases(dir.path()), vec![keep]);
}

#[test]
fn collecting_a_cache_that_was_never_created_is_a_no_op() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(collect(dir.path(), &[]).unwrap(), 0);
}
