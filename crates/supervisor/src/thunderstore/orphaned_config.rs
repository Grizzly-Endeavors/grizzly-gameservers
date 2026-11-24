use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const GUID_HEADER: &str = "## Plugin GUID:";

const HEADER_BYTES: usize = 4096;

const CONFIG_EXTENSION: &str = "cfg";

const ASSEMBLY_EXTENSION: &str = "dll";

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub removed: Vec<Orphan>,
    pub kept: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orphan {
    pub entry: String,
    pub guid: String,
}

impl std::fmt::Display for Orphan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.entry, self.guid)
    }
}

struct Candidate {
    entry: String,
    path: PathBuf,
    guid: String,
    is_dir: bool,
}

pub fn sweep(config_dir: &Path, staged_dir: &Path, keep: &[String]) -> Result<SweepReport> {
    if !staged_dir.is_dir() {
        return Ok(SweepReport::default());
    }
    let candidates = candidates(config_dir, keep)?;
    if candidates.is_empty() {
        return Ok(SweepReport::default());
    }

    let guids: Vec<String> = candidates
        .iter()
        .map(|candidate| candidate.guid.clone())
        .collect();
    let claimed = claimed_guids(staged_dir, &guids);

    let mut report = SweepReport::default();
    for candidate in candidates {
        if claimed.contains(&candidate.guid) {
            report.kept += 1;
            continue;
        }
        let removed = if candidate.is_dir {
            std::fs::remove_dir_all(&candidate.path)
        } else {
            std::fs::remove_file(&candidate.path)
        };
        match removed {
            Ok(()) => report.removed.push(Orphan {
                entry: candidate.entry,
                guid: candidate.guid,
            }),
            Err(err) => {
                tracing::warn!(
                    entry = %candidate.entry,
                    error = ?err,
                    "could not remove the settings of an uninstalled mod"
                );
                report.kept += 1;
            }
        }
    }
    Ok(report)
}

fn candidates(config_dir: &Path, keep: &[String]) -> Result<Vec<Candidate>> {
    let entries = match std::fs::read_dir(config_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to read {}", config_dir.display()));
        }
    };
    let mut candidates = Vec::new();
    for entry in entries.filter_map(std::result::Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if keep.contains(&name) {
            continue;
        }
        let path = entry.path();
        let is_dir = path.is_dir();
        let guid = if is_dir {
            directory_guid(&name).map(str::to_owned)
        } else {
            settings_file_guid(&path, &name)
        };
        if let Some(guid) = guid {
            candidates.push(Candidate {
                entry: name,
                path,
                guid,
                is_dir,
            });
        }
    }
    Ok(candidates)
}

fn settings_file_guid(path: &Path, name: &str) -> Option<String> {
    if !name.ends_with(&format!(".{CONFIG_EXTENSION}")) {
        return None;
    }
    let head = read_head(path)?;
    owning_guid(&head).map(str::to_owned)
}

fn read_head(path: &Path) -> Option<String> {
    use std::io::Read as _;

    let mut buffer = vec![0_u8; HEADER_BYTES];
    let mut file = std::fs::File::open(path).ok()?;
    let read = file.read(&mut buffer).ok()?;
    buffer.truncate(read);
    match String::from_utf8(buffer) {
        Ok(text) => Some(text),
        Err(err) => {
            let valid = err.utf8_error().valid_up_to();
            let bytes = err.into_bytes();
            bytes
                .get(..valid)
                .map(|head| String::from_utf8_lossy(head).into_owned())
        }
    }
}

fn owning_guid(head: &str) -> Option<&str> {
    head.lines()
        .find_map(|line| line.trim().strip_prefix(GUID_HEADER))
        .map(str::trim)
        .filter(|guid| !guid.is_empty())
}

fn directory_guid(name: &str) -> Option<&str> {
    (name.contains('.') && !name.starts_with('.')).then_some(name)
}

fn claimed_guids(staged_dir: &Path, guids: &[String]) -> Vec<String> {
    let mut claimed: Vec<String> = Vec::new();
    for assembly in assemblies(staged_dir) {
        let unclaimed: Vec<&String> = guids
            .iter()
            .filter(|guid| !claimed.contains(*guid))
            .collect();
        if unclaimed.is_empty() {
            break;
        }
        let Ok(bytes) = std::fs::read(&assembly) else {
            continue;
        };
        for guid in unclaimed {
            if contains(&bytes, guid.as_bytes()) {
                claimed.push(guid.clone());
            }
        }
    }
    claimed
}

fn assemblies(staged_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![staged_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(ASSEMBLY_EXTENSION))
            {
                found.push(path);
            }
        }
    }
    found
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((first, rest)) = needle.split_first() else {
        return false;
    };
    haystack
        .iter()
        .enumerate()
        .filter(|(_, byte)| *byte == first)
        .any(|(index, _)| {
            haystack
                .get(index + 1..)
                .is_some_and(|tail| tail.starts_with(rest))
        })
}

#[cfg(test)]
#[path = "tests/orphaned_config.rs"]
mod tests;
