//! The per-instance mod list: `.grizzly/mods`, one package per line.
//!
//! The file is the *group's modpack*, not just this server's install list. Two
//! forms of line, and the difference is which machines a package belongs on:
//!
//! - `Namespace-Name` (latest at resolve time) or `Namespace-Name-1.2.3`
//!   (pinned) — the server installs it;
//! - `client:Namespace-Name-1.2.3` — it belongs on the players' machines, and
//!   this server installs nothing for it.
//!
//! Blank lines and `#` comments are ignored. Trailing and leading whitespace is
//! trimmed.
//!
//! Unlike the env overlay next door, a bad *install* line is a **hard error**
//! that fails the launch. That asymmetry is deliberate and load-bearing: the
//! overlay degrades to "no overrides", which is a server that still boots, while
//! a typo in an install line would otherwise degrade to "that mod isn't
//! installed" — a silent, wrong, and durable answer to a friend who asked for
//! it. Failing loud routes the mistake into the bot's snapshot-restore path,
//! which puts the file back the way it was and restarts.
//!
//! A bad **client** line takes the opposite posture, for the mirror-image
//! reason: this server installs nothing for it, so it has no business taking the
//! world down. It is skipped with a warning that reaches the log buffer, and the
//! validation that would have caught it happens in the bot at write time, where
//! the registry is already being consulted.

use std::fmt;
use std::str::FromStr;

use super::types::{PackageId, PackageRelease, PackageVersion, ParseError};

/// Marks a line as belonging to the players' machines rather than this server.
/// One file, one diff, greppable — the same shape as Minecraft's `datapack:`
/// prefix in `MODRINTH_PROJECTS`.
const CLIENT_PREFIX: &str = "client:";

/// One package the server installs, optionally pinned to an exact version, and
/// the line it came from so every later failure can name it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsEntry {
    pub id: PackageId,
    /// The version the author pinned, or `None` to take the latest at resolve
    /// time. A pin is hard — nothing downstream raises it.
    pub pin: Option<PackageVersion>,
    /// 1-based line in the file, for error messages.
    pub line_number: usize,
}

/// One package the modpack carries for the players' machines only.
///
/// Always an exact release, never a bare id: nothing downstream resolves a
/// client entry, so no later step could supply the missing version — and a
/// version is exactly what the launcher on the other end needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientEntry {
    pub release: PackageRelease,
    pub line_number: usize,
}

/// The whole file, split by which machines each entry belongs on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModsList {
    pub install: Vec<ModsEntry>,
    pub client_only: Vec<ClientEntry>,
    /// Client lines that couldn't be used. Never empty *and* fatal — see the
    /// module docs.
    pub warnings: Vec<ModsFileWarning>,
}

/// Why a mod list couldn't be read. Every variant names its line, because the
/// friend fixing it is editing that file by line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModsFileError {
    Malformed {
        line_number: usize,
        line: String,
        reason: ParseError,
    },
    Duplicate {
        line_number: usize,
        id: PackageId,
        first_seen: usize,
    },
}

impl fmt::Display for ModsFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed {
                line_number,
                line,
                reason,
            } => write!(f, "line {line_number}: {line:?} is not a mod — {reason}"),
            Self::Duplicate {
                line_number,
                id,
                first_seen,
            } => write!(
                f,
                "line {line_number}: {id} is already listed on line {first_seen}; \
                 list each mod once"
            ),
        }
    }
}

impl std::error::Error for ModsFileError {}

/// A client line that was dropped. Reported rather than raised, so a mistake
/// about somebody else's machine can't stop this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModsFileWarning {
    /// Neither `client:Namespace-Name-1.2.3` nor anything close to it.
    Unusable {
        line_number: usize,
        line: String,
        reason: ParseError,
    },
    /// Well-formed but unpinned, which a launcher can do nothing with.
    Unpinned { line_number: usize, id: PackageId },
    /// The same package is already in the list.
    Duplicate {
        line_number: usize,
        id: PackageId,
        first_seen: usize,
    },
}

impl fmt::Display for ModsFileWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unusable {
                line_number,
                line,
                reason,
            } => write!(
                f,
                "line {line_number}: {line:?} is listed for players' machines but isn't usable \
                 — {reason}; skipping it"
            ),
            Self::Unpinned { line_number, id } => write!(
                f,
                "line {line_number}: {id} is listed for players' machines with no version, \
                 which a launcher can't use; skipping it"
            ),
            Self::Duplicate {
                line_number,
                id,
                first_seen,
            } => write!(
                f,
                "line {line_number}: {id} is already listed on line {first_seen}; \
                 skipping the players'-machines entry"
            ),
        }
    }
}

/// Parse the mod list.
///
/// # Errors
///
/// Returns the first [`ModsFileError`] an *install* line produces: a line that
/// is neither a package nor a pinned package, or a package already listed. Both
/// stop the launch rather than being skipped. Client lines never produce an
/// error — see the module docs.
pub fn parse(text: &str) -> Result<ModsList, ModsFileError> {
    let mut list = ModsList::default();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.strip_prefix(CLIENT_PREFIX) {
            Some(rest) => push_client(&mut list, rest.trim(), line_number),
            None => push_install(&mut list, line, line_number)?,
        }
    }
    Ok(list)
}

/// Record a line the server installs, or fail the launch on it.
fn push_install(list: &mut ModsList, line: &str, line_number: usize) -> Result<(), ModsFileError> {
    let entry = parse_install(line, line_number)?;
    if let Some(first_seen) = first_seen(list, &entry.id) {
        return Err(ModsFileError::Duplicate {
            line_number,
            id: entry.id,
            first_seen,
        });
    }
    list.install.push(entry);
    Ok(())
}

/// Record a line that belongs on the players' machines, or warn and drop it.
fn push_client(list: &mut ModsList, rest: &str, line_number: usize) {
    let release = match rest.parse::<PackageRelease>() {
        Ok(release) => release,
        Err(reason) => {
            // An unpinned but otherwise valid entry gets its own warning: the
            // fix is to add a version, not to re-type the whole line, and the
            // generic "not a release" message would send someone looking for a
            // typo that isn't there.
            list.warnings.push(match PackageId::from_str(rest) {
                Ok(id) => ModsFileWarning::Unpinned { line_number, id },
                Err(_) => ModsFileWarning::Unusable {
                    line_number,
                    line: rest.to_owned(),
                    reason,
                },
            });
            return;
        }
    };
    if let Some(first_seen) = first_seen(list, &release.id) {
        list.warnings.push(ModsFileWarning::Duplicate {
            line_number,
            id: release.id,
            first_seen,
        });
        return;
    }
    list.client_only.push(ClientEntry {
        release,
        line_number,
    });
}

/// The line `id` was first listed on, in either form. A package belongs to one
/// side or the other, so the duplicate rule spans both.
fn first_seen(list: &ModsList, id: &PackageId) -> Option<usize> {
    list.install
        .iter()
        .find(|entry| entry.id == *id)
        .map(|entry| entry.line_number)
        .or_else(|| {
            list.client_only
                .iter()
                .find(|entry| entry.release.id == *id)
                .map(|entry| entry.line_number)
        })
}

/// One install line. Tries the pinned shape first: a bare `Namespace-Name` never
/// parses as a release (a release needs a numeric third field), so the order
/// can't misread one as the other.
fn parse_install(line: &str, line_number: usize) -> Result<ModsEntry, ModsFileError> {
    if let Ok(release) = line.parse::<PackageRelease>() {
        return Ok(ModsEntry {
            id: release.id,
            pin: Some(release.version),
            line_number,
        });
    }
    match PackageId::from_str(line) {
        Ok(id) => Ok(ModsEntry {
            id,
            pin: None,
            line_number,
        }),
        // Reported against the pinned shape, since that is the form the guidance
        // tells Gary to write and the form a typo is most likely aiming at.
        Err(_) => Err(ModsFileError::Malformed {
            line_number,
            line: line.to_owned(),
            reason: ParseError::NotARelease,
        }),
    }
}

#[cfg(test)]
#[path = "tests/mods_file.rs"]
mod tests;
