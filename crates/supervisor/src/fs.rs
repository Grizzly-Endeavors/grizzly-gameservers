use std::path::{Component, Path, PathBuf};

use grizzly_control_api::{DirEntry, EntryKind};
use tracing::debug;

pub const READ_CAP_BYTES: usize = 256 * 1024;
pub const WRITE_CAP_BYTES: usize = 1024 * 1024;
const BACKUP_SUFFIX: &str = ".grizzly.bak";

/// Why a filesystem operation could not be served. Each maps to an HTTP status
/// at the control boundary and to a plain-language string for the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsError {
    OutsideRoot,
    NotFound,
    NotAFile,
    NotADirectory,
    NotText,
    TooLarge,
    NoBackup,
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

fn canonical_within(root: &Path, path: &Path) -> Result<PathBuf, FsError> {
    let root = root.canonicalize().map_err(|err| io(&err))?;
    let real = path.canonicalize().map_err(|err| io(&err))?;
    if real.starts_with(&root) {
        Ok(real)
    } else {
        Err(FsError::OutsideRoot)
    }
}

fn parent_within(root: &Path, path: &Path) -> Result<(), FsError> {
    let parent = path.parent().ok_or(FsError::OutsideRoot)?;
    canonical_within(root, parent).map(|_| ())
}

fn reject_escaping_symlink(root: &Path, path: &Path) -> Result<(), FsError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => canonical_within(root, path).map(|_| ()),
        Ok(_) | Err(_) => Ok(()),
    }
}

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
        let file_type = entry.file_type().map_err(|err| io(&err))?;
        let (kind, size_bytes) = if file_type.is_dir() {
            (EntryKind::Dir, 0)
        } else if file_type.is_file() {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileWindow {
    pub content: String,
    pub truncated: bool,
    pub line_cut: bool,
    pub start_line: usize,
    pub total_lines: usize,
}

pub const READ_LINE_CHARS: usize = 2000;

const LINE_CUT_MARKER: &str = "… [line truncated]";

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

    let first = start.max(1);
    let last = first.saturating_add(count);
    let mut content = String::new();
    let mut total_lines = 0_usize;
    let mut cut_a_line = false;
    let mut hit_byte_cap = false;

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
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
        start_line: 0,
        total_lines: 0,
    })
}

pub fn edit_file(root: &Path, rel: &str, old: &str, new: &str) -> Result<EditCount, FsError> {
    let resolved = resolve_in_root(root, rel)?;
    let canonical = canonical_within(root, &resolved)?;
    let meta = std::fs::metadata(&canonical).map_err(|err| io(&err))?;
    if meta.is_dir() {
        return Err(FsError::NotAFile);
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditCount {
    pub occurrences: usize,
    pub backed_up: bool,
}

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

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(BACKUP_SUFFIX);
    PathBuf::from(name)
}

#[cfg(test)]
#[path = "tests/fs.rs"]
mod tests;
