//! Removing the settings files a mod leaves behind when it is uninstalled.
//!
//! Deleting a line from the mod list removes the mod's *files* — the staged tree
//! is rebuilt from scratch and the image's hook mirrors it with `--delete`. Its
//! **settings** survive, deliberately: the same hook copies a package's config
//! defaults with `--ignore-existing` precisely so a value a friend tuned is
//! never reverted by a restart. The cost of that rule is that nothing ever
//! removes the settings of a mod that is gone, and the agent is the only reader
//! of this volume — so what accumulates is a directory where most files describe
//! mods that aren't installed, and a search for a setting lands on one of them.
//! Editing it looks like it worked and changes nothing.
//!
//! **Attribution comes from the files themselves, not from a manifest.** These
//! settings files are written by the loader at runtime, not shipped in the
//! package — so tracking what each package installed would catch none of them.
//! What makes them attributable is that the loader stamps the owning plugin's
//! GUID into a header line, and that same GUID string is embedded in the
//! assembly that declares it. So a settings file is orphaned when its GUID
//! appears in none of the staged assemblies, and that is decided by reading the
//! two artifacts rather than by remembering anything between launches.
//!
//! **Every ambiguity keeps the file.** A settings file with no GUID header is
//! the loader's own and is never a candidate; a GUID matched by any staged
//! assembly is live even if the match is incidental; an entry the image declares
//! as its own is skipped whatever the assemblies say. The sweep can fail to
//! remove something, which costs one stale file; it must not remove something
//! live, which costs a friend's settings.
//!
//! **It runs once the launch has proven good, not before it.** A mod list that
//! stops the server is restored and restarted by the bot, and a sweep that ran
//! ahead of that would have already deleted the settings of the mod the rollback
//! just put back. Waiting for the game to report ready means a rolled-back
//! change never reaches this code.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Header line the loader writes into every settings file it generates, naming
/// the plugin that owns it. The one durable link between a file and a package.
const GUID_HEADER: &str = "## Plugin GUID:";

/// How many bytes of a settings file are read looking for the header. The header
/// is the first thing in the file; this is a bound on a file a mod may have
/// grown to hundreds of kilobytes, not a guess at where the header is.
const HEADER_BYTES: usize = 4096;

/// Extension of a settings file. Matched exactly, so the `.cfg.grizzly.bak`
/// snapshots the file tools leave beside an edited file are not candidates —
/// those belong to the rollback path, which owns their lifetime.
const CONFIG_EXTENSION: &str = "cfg";

/// Extension of a staged file that can declare a plugin GUID. Narrowed to
/// assemblies both to keep the scan bounded and because the attribute carrying
/// the GUID exists nowhere else.
const ASSEMBLY_EXTENSION: &str = "dll";

/// What a sweep removed, and what it deliberately left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Entry names removed from the config directory, each with the GUID that
    /// nothing claimed.
    pub removed: Vec<Orphan>,
    /// Entry names that looked like plugin settings and were claimed by a staged
    /// assembly. Counted rather than named — this is the ordinary case.
    pub kept: usize,
}

/// One removed entry: what it was called and which plugin's settings it held.
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

/// One entry in the live config directory that a plugin might own.
struct Candidate {
    entry: String,
    path: PathBuf,
    guid: String,
    /// A whole directory of settings rather than a single file, removed as a
    /// unit because a plugin that keeps a directory keeps all of it.
    is_dir: bool,
}

/// Remove the settings of every mod that is no longer installed.
///
/// `config_dir` is the live directory the game's loader reads settings from and
/// `staged_dir` the reconciler's staged install tree. `keep` names entries the
/// image owns — settings for a plugin the image bakes in, whose assembly is not
/// in the staged tree and which would otherwise look orphaned.
///
/// A missing staged tree is not an empty one: it means the reconciler has not
/// run, so nothing here can say what is installed and the sweep does nothing.
///
/// # Errors
///
/// Returns an error if the config directory exists but can't be read. An
/// individual entry that can't be read or removed is left in place and reported
/// as kept: a stale file costs a confusing read, and failing the caller over one
/// would abandon the rest of the sweep.
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
            // Reported as kept rather than raised: the next launch tries again,
            // and the caller's job is to say what the directory now holds.
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

/// Every entry in the config directory that carries a plugin GUID, skipping the
/// ones the image declares as its own.
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

/// The GUID a settings file declares, or `None` when it declares none — which is
/// how the loader's own configuration is left alone without naming it.
fn settings_file_guid(path: &Path, name: &str) -> Option<String> {
    if !name.ends_with(&format!(".{CONFIG_EXTENSION}")) {
        return None;
    }
    let head = read_head(path)?;
    owning_guid(&head).map(str::to_owned)
}

/// Read the front of a file as text, stopping at the last complete UTF-8
/// character so a binary tail can't fail the read.
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

/// The plugin GUID a settings file's header names.
fn owning_guid(head: &str) -> Option<&str> {
    head.lines()
        .find_map(|line| line.trim().strip_prefix(GUID_HEADER))
        .map(str::trim)
        .filter(|guid| !guid.is_empty())
}

/// The GUID a settings *directory* is named after.
///
/// A plugin that keeps more than one file names the directory after its GUID,
/// which is the only attribution a directory can carry. Requiring a dot is what
/// keeps a directory that is plainly not a GUID (`plugins`) out of the sweep
/// entirely rather than relying on an incidental match to save it.
fn directory_guid(name: &str) -> Option<&str> {
    (name.contains('.') && !name.starts_with('.')).then_some(name)
}

/// Which of `guids` any staged assembly declares.
///
/// Scans the assemblies rather than asking them: the GUID is an attribute
/// argument, stored in the assembly as the literal string, so its presence is
/// exactly the question and reading the metadata properly would answer nothing
/// more.
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

/// Every assembly under the staged tree, at any depth.
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

/// Whether `needle` appears anywhere in `haystack`.
///
/// Hand-rolled rather than pulled in: the candidate first byte is checked before
/// the rest is compared, which is what keeps a pass over a multi-megabyte
/// assembly cheap without a substring-search dependency.
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
