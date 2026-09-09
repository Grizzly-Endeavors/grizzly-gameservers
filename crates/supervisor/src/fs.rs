//! Path-scoped filesystem operations the control API exposes for the ops agent.
//!
//! Every operation is confined to the supervisor's data root (the instance PVC
//! mount). [`resolve_in_root`] is the lexical guard — it rejects absolute paths
//! and `..` escapes before any IO — and each operation additionally canonicalizes
//! the touched path to defeat symlinks that point back out of the root. This is
//! the in-pod blast-radius boundary: a confused or prompt-injected agent can read
//! and write the game's own data, and nothing else.

use std::path::{Component, Path, PathBuf};

use grizzly_control_api::{DirEntry, EntryKind};
use tracing::debug;

/// Cap on bytes returned by [`read_file`]; larger files come back truncated so a
/// multi-megabyte world or log file can't blow up the agent's context window.
pub const READ_CAP_BYTES: usize = 256 * 1024;
/// Cap on bytes accepted by [`write_file`]. Config files are small; this is a
/// guard against a runaway write, not a real workload limit.
pub const WRITE_CAP_BYTES: usize = 1024 * 1024;
/// Suffix of the snapshot [`write_file`] takes before overwriting, and that
/// [`restore_file`] reads back. Lives beside the file on the same PVC.
const BACKUP_SUFFIX: &str = ".grizzly.bak";

/// Why a filesystem operation could not be served. Each maps to an HTTP status
/// at the control boundary and to a plain-language string for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    /// The path was absolute, contained `..`, or resolved (via a symlink) to
    /// somewhere outside the data root.
    OutsideRoot,
    /// Nothing exists at the path.
    NotFound,
    /// A file operation was asked for on a directory.
    NotAFile,
    /// A directory operation was asked for on a file.
    NotADirectory,
    /// The file is not valid UTF-8 text, so it isn't safe to hand to the agent.
    NotText,
    /// The write payload exceeds [`WRITE_CAP_BYTES`].
    TooLarge,
    /// A restore was asked for but no snapshot exists.
    NoBackup,
    /// An underlying IO error, carried as its display string for logging.
    Io(String),
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutsideRoot => f.write_str("path is outside the server's data directory"),
            Self::NotFound => f.write_str("no such file or directory"),
            Self::NotAFile => f.write_str("path is a directory, not a file"),
            Self::NotADirectory => f.write_str("path is a file, not a directory"),
            Self::NotText => f.write_str("file is not text"),
            Self::TooLarge => f.write_str("content is too large to write"),
            Self::NoBackup => f.write_str("no saved version to restore"),
            Self::Io(message) => write!(f, "io error: {message}"),
        }
    }
}

impl FsError {
    /// Whether the error is the caller's fault (bad path / bad request) rather
    /// than an internal IO failure — drives 4xx vs 5xx at the HTTP boundary.
    #[must_use]
    pub const fn is_client_error(&self) -> bool {
        !matches!(self, Self::Io(_))
    }
}

fn io(err: &std::io::Error) -> FsError {
    if err.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound
    } else {
        FsError::Io(err.to_string())
    }
}

/// Lexically resolve a data-root-relative request path against `root`, rejecting
/// anything that could escape it. Pure: no IO, no symlink following — the
/// symlink defense lives in [`canonical_within`], applied by each operation.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`] if `rel` is absolute, carries a drive prefix,
/// or contains a `..` component.
pub fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, FsError> {
    let mut normalized = PathBuf::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::OutsideRoot);
            }
        }
    }
    Ok(root.join(normalized))
}

/// Confirm that `path` (already lexically resolved) canonicalizes to somewhere
/// under `root`, defeating a symlink inside the root that points back out. Used
/// for paths that must already exist (reads, lists, restores).
fn canonical_within(root: &Path, path: &Path) -> Result<PathBuf, FsError> {
    let root = root.canonicalize().map_err(|err| io(&err))?;
    let real = path.canonicalize().map_err(|err| io(&err))?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(FsError::OutsideRoot)
    }
}

/// Confirm that the *parent* of `path` is under `root`, for a write whose target
/// file may not exist yet (so `path` itself can't be canonicalized).
fn parent_within(root: &Path, path: &Path) -> Result<(), FsError> {
    let parent = path.parent().ok_or(FsError::OutsideRoot)?;
    canonical_within(root, parent).map(|_| ())
}

/// Reject `path` if a symlink already sits there pointing outside `root`.
/// `parent_within` only vets the parent directory, so a write/restore target
/// that doesn't exist yet can't be `canonical_within`-checked directly — but a
/// *pre-existing* symlink at that exact path must still be caught before
/// `fs::write`/`fs::copy` follows it out of the root. A target that doesn't
/// exist, or exists but isn't a symlink, is safe: its real path is exactly
/// `parent`'s (already-verified) real path joined with the file name.
fn reject_escaping_symlink(root: &Path, path: &Path) -> Result<(), FsError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => canonical_within(root, path).map(|_| ()),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// Create an empty file at `rel` under `root`, and its parent directory, if it
/// isn't there already. Returns the resolved absolute path, so a caller that
/// goes on to read the file uses the same root-confined resolution rather than
/// re-joining `rel` itself (a plain `join` on an absolute `rel` would silently
/// discard the root).
///
/// The point is not the file, it is that it *exists before anyone edits it*:
/// [`write_file`] only snapshots a file that already existed, and the bot's
/// auto-rollback only arms for writes that took a snapshot. A reserved file that
/// is seeded empty is therefore rollback-eligible from its very first edit,
/// which is what makes a bad first mod list recoverable. Idempotent, and safe to
/// call before every launch: `create_new` means it can never truncate a file
/// somebody authored.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`] if `rel` escapes the data root, or
/// [`FsError::Io`] if the directory or file can't be created.
pub fn seed_empty_file(root: &Path, rel: &str) -> Result<PathBuf, FsError> {
    let path = resolve_in_root(root, rel)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io(&err))?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(_) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Ok(path),
        Err(err) => Err(io(&err)),
    }
}

/// List the entries of a directory under `root`, sorted by name.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`] for an escaping path, [`FsError::NotFound`]
/// if it doesn't exist, [`FsError::NotADirectory`] if it's a file, or
/// [`FsError::Io`] on an underlying read failure.
pub fn list_dir(root: &Path, rel: &str) -> Result<Vec<DirEntry>, FsError> {
    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if !meta.is_dir() {
        return Err(FsError::NotADirectory);
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&canonical).map_err(|err| io(&err))? {
        let entry = entry.map_err(|err| io(&err))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // `file_type()` reads the dir entry itself, so it doesn't follow symlinks.
        let file_type = entry.file_type().map_err(|err| io(&err))?;
        let (kind, size_bytes) = if file_type.is_dir() {
            (EntryKind::Dir, 0)
        } else if file_type.is_file() {
            // Size comes from a separate stat. If the file vanishes between the
            // read_dir and this call (a concurrent delete), degrade to 0 and log
            // rather than fail the whole listing over one racing entry.
            let len = match entry.metadata() {
                Ok(entry_meta) => entry_meta.len(),
                Err(err) => {
                    debug!(error = ?err, name, "could not stat directory entry; reporting size 0");
                    0
                }
            };
            (EntryKind::File, len)
        } else {
            (EntryKind::Other, 0)
        };
        entries.push(DirEntry {
            name,
            kind,
            size_bytes,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// One window of a file's bytes, decoded as UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWindow {
    /// The decoded text of the window.
    pub content: String,
    /// Whether more of the file remains than came back.
    pub truncated: bool,
    /// Whether any line in the window was cut at [`READ_LINE_CHARS`].
    pub line_cut: bool,
    /// 1-based line the window starts at, or `0` for a byte-addressed read,
    /// whose start doesn't correspond to a line boundary.
    pub start_line: usize,
    /// Lines in the whole file, not in this window — what tells a caller how
    /// much it hasn't seen. `0` for a byte-addressed read, which never counts.
    pub total_lines: usize,
}

/// Characters of a single line returned before it is cut short. One minified
/// JSON config is a single line of hundreds of kilobytes, and without this it
/// would consume a whole line window on its own.
pub const READ_LINE_CHARS: usize = 2000;

/// Marker appended to a line cut at [`READ_LINE_CHARS`], so a truncated line
/// can't be mistaken for the file's actual content.
const LINE_CUT_MARKER: &str = "… [line truncated]";

/// Read a window of `count` lines from `rel`, starting at 1-based `start`.
///
/// Streams rather than loading the file, so a large file costs IO instead of
/// memory, and counts the whole file's lines on the way through so the caller
/// learns what it hasn't seen. A `start` past the end of the file reads as an
/// empty window rather than an error — the natural end of a paged read.
///
/// Each line is cut at [`READ_LINE_CHARS`], and the window as a whole is still
/// bounded by [`READ_CAP_BYTES`]; `truncated` covers all three ways a window can
/// fall short of the file — more lines after it, a line cut short, or the byte
/// cap reached.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::NotFound`], [`FsError::NotAFile`]
/// for a directory, [`FsError::NotText`] for non-UTF-8 bytes, or [`FsError::Io`].
pub fn read_file_lines(
    root: &Path,
    rel: &str,
    start: usize,
    count: usize,
) -> Result<FileWindow, FsError> {
    use std::io::BufRead as _;

    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if meta.is_dir() {
        return Err(FsError::NotAFile);
    }
    let file = std::fs::File::open(&canonical).map_err(|err| io(&err))?;
    let reader = std::io::BufReader::new(file);

    // A `start` of zero would otherwise silently mean "line one"; 1-based is the
    // contract, and clamping keeps an off-by-one caller reading the file's head
    // rather than erroring on a detail it can't see.
    let first = start.max(1);
    let last = first.saturating_add(count);
    let mut content = String::new();
    let mut total_lines = 0_usize;
    let mut cut_a_line = false;
    let mut hit_byte_cap = false;

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            // An invalid byte anywhere means this isn't a text file, which is the
            // same verdict the byte-addressed read reaches; `InvalidData` is how
            // `BufRead::lines` reports it.
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => {
                return Err(FsError::NotText);
            }
            Err(err) => return Err(io(&err)),
        };
        total_lines = total_lines.saturating_add(1);
        if total_lines < first || total_lines >= last || hit_byte_cap {
            continue;
        }
        let kept = match line.char_indices().nth(READ_LINE_CHARS) {
            Some((cut, _)) => {
                cut_a_line = true;
                let head = line.get(..cut).unwrap_or(&line);
                format!("{head}{LINE_CUT_MARKER}")
            }
            None => line,
        };
        if content.len().saturating_add(kept.len()) > READ_CAP_BYTES {
            hit_byte_cap = true;
            continue;
        }
        content.push_str(&kept);
        content.push('\n');
    }

    let more_lines = total_lines >= last;
    Ok(FileWindow {
        content,
        truncated: more_lines || hit_byte_cap,
        line_cut: cut_a_line,
        start_line: first,
        total_lines,
    })
}

/// Read up to [`READ_CAP_BYTES`] of a UTF-8 file under `root`.
///
/// The whole file, for a caller that will parse it. Anything read for a person
/// or a model wants [`read_file_lines`] instead, which bounds what comes back
/// and says where it sits in the file.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::NotFound`], [`FsError::NotAFile`]
/// for a directory, [`FsError::NotText`] for non-UTF-8 bytes, or [`FsError::Io`].
pub fn read_file(root: &Path, rel: &str) -> Result<FileWindow, FsError> {
    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if meta.is_dir() {
        return Err(FsError::NotAFile);
    }
    let bytes = std::fs::read(&canonical).map_err(|err| io(&err))?;
    let truncated = bytes.len() > READ_CAP_BYTES;
    let capped = bytes.get(..READ_CAP_BYTES).unwrap_or(&bytes);
    let content = match std::str::from_utf8(capped) {
        Ok(text) => text.to_owned(),
        // Cutting at the cap can split a multibyte char, which surfaces as an
        // *incomplete* trailing sequence (`error_len() == None`). Keep the valid
        // prefix rather than mislabel a real UTF-8 file as binary — a genuine
        // invalid byte (`error_len() == Some`) is really not text.
        Err(err) if truncated && err.error_len().is_none() => {
            let valid = capped.get(..err.valid_up_to()).unwrap_or(capped);
            String::from_utf8_lossy(valid).into_owned()
        }
        Err(_) => return Err(FsError::NotText),
    };
    Ok(FileWindow {
        content,
        truncated,
        line_cut: false,
        // Not addressed in lines, and counting them is work this caller didn't
        // ask for.
        start_line: 0,
        total_lines: 0,
    })
}

/// Replace the single occurrence of `old` with `new` in a file under `root`,
/// snapshotting the prior version first exactly as [`write_file`] does.
///
/// Returns how many times `old` occurred. Anything but exactly one leaves the
/// file untouched: zero has no location to write at, and more than one would mean
/// guessing which the caller meant. Only the count comes back, so a caller learns
/// why an edit didn't land without the file's contents crossing the wire.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::NotFound`], [`FsError::NotAFile`]
/// for a directory, [`FsError::NotText`] for non-UTF-8 bytes, [`FsError::TooLarge`]
/// if the result would exceed [`WRITE_CAP_BYTES`], or [`FsError::Io`].
pub fn edit_file(root: &Path, rel: &str, old: &str, new: &str) -> Result<EditCount, FsError> {
    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if meta.is_dir() {
        return Err(FsError::NotAFile);
    }
    // Checked before the read, not after: an edit has to write the whole file
    // back, so a file already past the write cap can never be edited, and
    // learning that from its metadata avoids pulling it into memory to find out.
    if meta.len() > WRITE_CAP_BYTES as u64 {
        return Err(FsError::TooLarge);
    }
    let bytes = std::fs::read(&canonical).map_err(|err| io(&err))?;
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Err(FsError::NotText);
    };

    let occurrences = text.matches(old).count();
    if occurrences != 1 {
        return Ok(EditCount {
            occurrences,
            backed_up: false,
        });
    }
    let updated = text.replacen(old, new, 1);
    if updated.len() > WRITE_CAP_BYTES {
        return Err(FsError::TooLarge);
    }
    let backed_up = write_file(root, rel, &updated)?;
    Ok(EditCount {
        occurrences,
        backed_up,
    })
}

/// What an [`edit_file`] attempt did: how many times the anchor matched, and
/// whether a snapshot was taken (only ever on the single-match path that wrote).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditCount {
    pub occurrences: usize,
    pub backed_up: bool,
}

/// Overwrite a file under `root`, snapshotting any existing version first.
///
/// Returns whether a snapshot was taken (`true` when the file already existed).
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::TooLarge`] past
/// [`WRITE_CAP_BYTES`], [`FsError::NotAFile`] if the target is a directory, or
/// [`FsError::Io`].
pub fn write_file(root: &Path, rel: &str, content: &str) -> Result<bool, FsError> {
    if content.len() > WRITE_CAP_BYTES {
        return Err(FsError::TooLarge);
    }
    let resolved = resolve_in_root(root, rel)?;
    parent_within(root, &resolved)?;
    reject_escaping_symlink(root, &resolved)?;
    let backup = backup_path(&resolved);
    reject_escaping_symlink(root, &backup)?;
    let backed_up = match std::fs::metadata(&resolved) {
        Ok(meta) if meta.is_dir() => return Err(FsError::NotAFile),
        Ok(_) => {
            std::fs::copy(&resolved, &backup).map_err(|err| io(&err))?;
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(io(&err)),
    };
    std::fs::write(&resolved, content).map_err(|err| io(&err))?;
    Ok(backed_up)
}

/// Restore a file under `root` from the snapshot the last [`write_file`] took.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::NoBackup`] if no snapshot exists,
/// or [`FsError::Io`].
pub fn restore_file(root: &Path, rel: &str) -> Result<(), FsError> {
    let resolved = resolve_in_root(root, rel)?;
    parent_within(root, &resolved)?;
    reject_escaping_symlink(root, &resolved)?;
    let backup = backup_path(&resolved);
    reject_escaping_symlink(root, &backup)?;
    match std::fs::copy(&backup, &resolved) {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(FsError::NoBackup),
        Err(err) => Err(io(&err)),
    }
}

/// The snapshot path for a target file: its own path with [`BACKUP_SUFFIX`]
/// appended, so it sits beside the original on the same PVC.
fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

#[cfg(test)]
#[path = "tests/fs.rs"]
mod tests;
