use std::path::Path;

use tempfile::TempDir;

use super::*;

fn seed() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("server.properties"), "difficulty=normal\n").unwrap();
    std::fs::create_dir(root.join("logs")).unwrap();
    std::fs::write(root.join("logs").join("latest.log"), "started\nready\n").unwrap();
    dir
}

#[test]
fn resolve_accepts_nested_relative_paths() {
    let resolved = resolve_in_root(Path::new("/data"), "logs/latest.log").unwrap();
    assert_eq!(resolved, Path::new("/data/logs/latest.log"));
}

#[test]
fn resolve_collapses_curdir_components() {
    let resolved = resolve_in_root(Path::new("/data"), "./logs/./latest.log").unwrap();
    assert_eq!(resolved, Path::new("/data/logs/latest.log"));
}

#[test]
fn resolve_rejects_parent_traversal() {
    assert_eq!(
        resolve_in_root(Path::new("/data"), "../etc/passwd"),
        Err(FsError::OutsideRoot),
        "a leading .. must be rejected"
    );
    assert_eq!(
        resolve_in_root(Path::new("/data"), "logs/../../etc/passwd"),
        Err(FsError::OutsideRoot),
        "a .. buried mid-path must be rejected"
    );
}

#[test]
fn resolve_rejects_absolute_paths() {
    assert_eq!(
        resolve_in_root(Path::new("/data"), "/etc/passwd"),
        Err(FsError::OutsideRoot),
        "an absolute path must be rejected"
    );
}

#[test]
fn list_dir_returns_sorted_entries_with_kinds() {
    let dir = seed();
    let entries = list_dir(dir.path(), "").unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["logs", "server.properties"],
        "entries should be sorted by name"
    );
    let logs = entries.iter().find(|e| e.name == "logs").unwrap();
    assert_eq!(logs.kind, EntryKind::Dir);
    let props = entries
        .iter()
        .find(|e| e.name == "server.properties")
        .unwrap();
    assert_eq!(props.kind, EntryKind::File);
    assert_eq!(props.size, "difficulty=normal\n".len() as u64);
}

#[test]
fn list_dir_on_a_file_is_not_a_directory() {
    let dir = seed();
    assert_eq!(
        list_dir(dir.path(), "server.properties"),
        Err(FsError::NotADirectory)
    );
}

#[test]
fn read_file_returns_content_untruncated() {
    let dir = seed();
    let (content, truncated) = read_file(dir.path(), "server.properties").unwrap();
    assert_eq!(content, "difficulty=normal\n");
    assert!(!truncated, "a small file should not be truncated");
}

#[test]
fn read_file_truncates_past_the_cap() {
    let dir = seed();
    let big = "a".repeat(READ_CAP_BYTES + 10);
    std::fs::write(dir.path().join("big.txt"), &big).unwrap();
    let (content, truncated) = read_file(dir.path(), "big.txt").unwrap();
    assert_eq!(content.len(), READ_CAP_BYTES, "read should be capped");
    assert!(truncated, "an oversized file should report truncation");
}

#[test]
fn read_file_rejects_non_utf8() {
    let dir = seed();
    std::fs::write(dir.path().join("world.dat"), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(read_file(dir.path(), "world.dat"), Err(FsError::NotText));
}

#[test]
fn read_file_on_a_directory_is_not_a_file() {
    let dir = seed();
    assert_eq!(read_file(dir.path(), "logs"), Err(FsError::NotAFile));
}

#[test]
fn read_missing_file_is_not_found() {
    let dir = seed();
    assert_eq!(read_file(dir.path(), "nope.txt"), Err(FsError::NotFound));
}

#[test]
fn write_snapshots_existing_file_then_overwrites() {
    let dir = seed();
    let backed_up = write_file(dir.path(), "server.properties", "difficulty=hard\n").unwrap();
    assert!(backed_up, "an existing file should be snapshotted");
    let (content, _) = read_file(dir.path(), "server.properties").unwrap();
    assert_eq!(content, "difficulty=hard\n", "the new content should land");
    let backup = std::fs::read_to_string(dir.path().join("server.properties.grizzly.bak")).unwrap();
    assert_eq!(
        backup, "difficulty=normal\n",
        "the snapshot should hold the prior content"
    );
}

#[test]
fn write_new_file_reports_no_backup() {
    let dir = seed();
    let backed_up = write_file(dir.path(), "ops.json", "{}").unwrap();
    assert!(!backed_up, "a fresh file has nothing to snapshot");
    let (content, _) = read_file(dir.path(), "ops.json").unwrap();
    assert_eq!(content, "{}");
}

#[test]
fn write_rejects_oversized_content() {
    let dir = seed();
    let huge = "x".repeat(WRITE_CAP_BYTES + 1);
    assert_eq!(
        write_file(dir.path(), "server.properties", &huge),
        Err(FsError::TooLarge)
    );
}

#[test]
fn write_rejects_escaping_path() {
    let dir = seed();
    assert_eq!(
        write_file(dir.path(), "../escape.txt", "nope"),
        Err(FsError::OutsideRoot)
    );
}

#[test]
fn restore_brings_back_the_snapshot() {
    let dir = seed();
    write_file(dir.path(), "server.properties", "difficulty=hard\n").unwrap();
    restore_file(dir.path(), "server.properties").unwrap();
    let (content, _) = read_file(dir.path(), "server.properties").unwrap();
    assert_eq!(
        content, "difficulty=normal\n",
        "restore should return the pre-write content"
    );
}

#[test]
fn restore_without_a_snapshot_errors() {
    let dir = seed();
    assert_eq!(
        restore_file(dir.path(), "server.properties"),
        Err(FsError::NoBackup)
    );
}

#[test]
fn symlink_escape_is_rejected_on_read() {
    let dir = seed();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret"), "top secret").unwrap();
    // A symlink that lives inside the root but points outside it.
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
    assert_eq!(read_file(dir.path(), "link"), Err(FsError::OutsideRoot));
}

#[test]
fn symlink_escape_is_rejected_on_write() {
    let dir = seed();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret");
    std::fs::write(&secret, "untouched").unwrap();
    // A symlink that lives inside the root but points outside it — the write
    // target itself, not just its parent directory.
    std::os::unix::fs::symlink(&secret, dir.path().join("link")).unwrap();
    assert_eq!(
        write_file(dir.path(), "link", "pwned"),
        Err(FsError::OutsideRoot)
    );
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "untouched",
        "the write must never reach the symlink target"
    );
}

#[test]
fn symlink_escape_is_rejected_on_restore() {
    let dir = seed();
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("secret");
    std::fs::write(&secret, "top secret").unwrap();
    // The restore *target* is a symlink pointing outside the root.
    std::os::unix::fs::symlink(&secret, dir.path().join("link")).unwrap();
    std::fs::write(dir.path().join("link.grizzly.bak"), "backup content").unwrap();
    assert_eq!(restore_file(dir.path(), "link"), Err(FsError::OutsideRoot));
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "top secret",
        "the restore must never overwrite the symlink target"
    );
}

#[test]
fn client_errors_classify_as_4xx_and_io_as_5xx() {
    assert!(FsError::OutsideRoot.is_client_error());
    assert!(FsError::NotFound.is_client_error());
    assert!(!FsError::Io("disk".to_owned()).is_client_error());
}
