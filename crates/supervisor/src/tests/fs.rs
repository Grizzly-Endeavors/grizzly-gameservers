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
    assert_eq!(props.size_bytes, "difficulty=normal\n".len() as u64);
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
    let window = read_file(dir.path(), "server.properties", 0).unwrap();
    let (content, truncated) = (window.content, window.truncated);
    assert_eq!(content, "difficulty=normal\n");
    assert!(!truncated, "a small file should not be truncated");
}

#[test]
fn read_file_truncates_past_the_cap() {
    let dir = seed();
    let big = "a".repeat(READ_CAP_BYTES + 10);
    std::fs::write(dir.path().join("big.txt"), &big).unwrap();
    let window = read_file(dir.path(), "big.txt", 0).unwrap();
    let (content, truncated) = (window.content, window.truncated);
    assert_eq!(content.len(), READ_CAP_BYTES, "read should be capped");
    assert!(truncated, "an oversized file should report truncation");
}

#[test]
fn read_file_truncation_keeps_valid_text_when_a_char_straddles_the_cap() {
    // A genuinely-UTF-8 file whose multibyte char sits astride READ_CAP_BYTES must
    // still read as (truncated) text, not be mislabeled binary. Place a 3-byte '€'
    // so its first byte is the last inside the cap and the rest spills past it.
    let dir = seed();
    let mut content = "a".repeat(READ_CAP_BYTES - 1);
    content.push('€');
    content.push_str("tail");
    std::fs::write(dir.path().join("unicode.txt"), &content).unwrap();
    let window = read_file(dir.path(), "unicode.txt", 0).unwrap();
    let (text, truncated) = (window.content, window.truncated);
    assert!(truncated, "an oversized file should report truncation");
    assert_eq!(
        text.len(),
        READ_CAP_BYTES - 1,
        "the prefix stops at the last whole char before the cap"
    );
    assert!(
        text.bytes().all(|b| b == b'a'),
        "only the valid ascii prefix survives; the split char is dropped"
    );
}

#[test]
fn read_file_rejects_non_utf8() {
    let dir = seed();
    std::fs::write(dir.path().join("world.dat"), [0xff, 0xfe, 0x00]).unwrap();
    assert_eq!(read_file(dir.path(), "world.dat", 0), Err(FsError::NotText));
}

#[test]
fn read_file_on_a_directory_is_not_a_file() {
    let dir = seed();
    assert_eq!(read_file(dir.path(), "logs", 0), Err(FsError::NotAFile));
}

#[test]
fn read_missing_file_is_not_found() {
    let dir = seed();
    assert_eq!(read_file(dir.path(), "nope.txt", 0), Err(FsError::NotFound));
}

#[test]
fn write_snapshots_existing_file_then_overwrites() {
    let dir = seed();
    let backed_up = write_file(dir.path(), "server.properties", "difficulty=hard\n").unwrap();
    assert!(backed_up, "an existing file should be snapshotted");
    let window = read_file(dir.path(), "server.properties", 0).unwrap();
    let (content, _) = (window.content, window.truncated);
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
    let window = read_file(dir.path(), "ops.json", 0).unwrap();
    let (content, _) = (window.content, window.truncated);
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
    let window = read_file(dir.path(), "server.properties", 0).unwrap();
    let (content, _) = (window.content, window.truncated);
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
    assert_eq!(read_file(dir.path(), "link", 0), Err(FsError::OutsideRoot));
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

#[test]
fn seeding_creates_the_file_and_its_parent_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = seed_empty_file(dir.path(), "nested/dir/reserved").unwrap();
    assert!(path.is_file());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn seeding_an_existing_file_never_truncates_it() {
    // This runs before every launch, not just the first: truncating would erase
    // a file the friend authored on the next restart.
    let dir = tempfile::tempdir().unwrap();
    let path = seed_empty_file(dir.path(), "reserved").unwrap();
    std::fs::write(&path, "authored content").unwrap();
    seed_empty_file(dir.path(), "reserved").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "authored content");
}

#[test]
fn seeding_refuses_a_path_outside_the_data_root() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        seed_empty_file(dir.path(), "../escape"),
        Err(FsError::OutsideRoot)
    );
    assert_eq!(
        seed_empty_file(dir.path(), "/etc/passwd"),
        Err(FsError::OutsideRoot)
    );
}

#[test]
fn a_seeded_file_is_snapshotted_on_its_very_first_edit() {
    // The whole point of seeding: write_file only snapshots a file that already
    // existed, and the bot's auto-rollback only arms for writes that took a
    // snapshot. Without the seed the first mod list would be unrecoverable.
    let dir = tempfile::tempdir().unwrap();
    seed_empty_file(dir.path(), "reserved").unwrap();
    let backed_up = write_file(dir.path(), "reserved", "ValheimModding-Jotunn\n").unwrap();
    assert!(
        backed_up,
        "the first edit of a seeded file must be rollback-eligible"
    );
}

#[test]
fn an_offset_reads_from_the_middle_of_a_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "abcdefghij").unwrap();
    let window = read_file(dir.path(), "notes.txt", 4).unwrap();
    assert_eq!(window.content, "efghij");
    assert_eq!(window.offset, 4);
    assert!(!window.truncated);
}

#[test]
fn paging_with_the_reported_offset_reaches_the_end_of_a_long_file() {
    // The point of the offset: a file past the read cap used to be unreachable
    // beyond its first window, because the truncation is a head.
    let dir = tempfile::tempdir().unwrap();
    let body = "x".repeat(READ_CAP_BYTES + 512);
    std::fs::write(dir.path().join("big.txt"), &body).unwrap();
    let first = read_file(dir.path(), "big.txt", 0).unwrap();
    assert!(first.truncated);
    let next = first.offset + first.content.len();
    let second = read_file(dir.path(), "big.txt", next).unwrap();
    assert!(!second.truncated);
    assert_eq!(second.content.len(), 512);
    assert_eq!(first.content.len() + second.content.len(), body.len());
}

#[test]
fn an_offset_inside_a_character_resumes_on_a_boundary() {
    // Paging must not turn an ordinary UTF-8 file into "not text" just because
    // a window boundary landed mid-character.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("unicode.txt"), "héllo").unwrap();
    // 'é' occupies bytes 1..3, so this offset lands on its continuation byte.
    let window = read_file(dir.path(), "unicode.txt", 2).unwrap();
    assert_eq!(window.content, "llo");
    assert_eq!(window.offset, 3);
}

#[test]
fn an_offset_past_the_end_reads_empty_rather_than_failing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "short").unwrap();
    let window = read_file(dir.path(), "notes.txt", 9_000).unwrap();
    assert!(window.content.is_empty());
    assert!(!window.truncated);
}
