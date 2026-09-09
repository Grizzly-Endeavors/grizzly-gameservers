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
    /// Whether bytes remain after this window.
    pub truncated: bool,
    /// The byte offset the window actually starts at. May sit slightly past a
    /// requested offset that landed inside a multi-byte character, which is why
    /// it is reported rather than assumed: a caller paging with
    /// `offset + content.len()` needs the real start to make progress.
    pub offset: usize,
}

/// Read up to [`READ_CAP_BYTES`] of a UTF-8 file under `root`, starting at
/// `offset` bytes in.
///
/// An offset past the end of the file reads as an empty, untruncated window
/// rather than an error — that is the natural end of a paged read, not a
/// mistake worth reporting as one.
///
/// # Errors
///
/// Returns [`FsError::OutsideRoot`], [`FsError::NotFound`], [`FsError::NotAFile`]
/// for a directory, [`FsError::NotText`] for non-UTF-8 bytes, or [`FsError::Io`].
pub fn read_file(root: &Path, rel: &str, offset: usize) -> Result<FileWindow, FsError> {
    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if meta.is_dir() {
        return Err(FsError::NotAFile);
    }
    let bytes = std::fs::read(&canonical).map_err(|err| io(&err))?;
    let start = offset.min(bytes.len());
    let window = bytes.get(start..).unwrap_or(&[]);
    let truncated = window.len() > READ_CAP_BYTES;
    let capped = window.get(..READ_CAP_BYTES).unwrap_or(window);
    // An offset can land inside a multi-byte character, leaving the window
    // opening on continuation bytes that are not a character on their own. Drop
    // them rather than call an ordinary UTF-8 file binary.
    let skipped = if start > 0 {
        capped
            .iter()
            .take_while(|byte| (**byte & 0b1100_0000) == 0b1000_0000)
            .count()
    } else {
        0
    };
    let slice = capped.get(skipped..).unwrap_or(&[]);
    let content = match std::str::from_utf8(slice) {
        Ok(text) => text.to_owned(),
        // Cutting at the cap can split a multibyte char, which surfaces as an
        // *incomplete* trailing sequence (`error_len() == None`). Keep the valid
        // prefix rather than mislabel a real UTF-8 file as binary — a genuine
        // invalid byte (`error_len() == Some`) is really not text.
        Err(err) if truncated && err.error_len().is_none() => {
            let valid = slice.get(..err.valid_up_to()).unwrap_or(slice);
            String::from_utf8_lossy(valid).into_owned()
        }
        Err(_) => return Err(FsError::NotText),
    };
    Ok(FileWindow {
        content,
        truncated,
        offset: start.saturating_add(skipped),
    })
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
