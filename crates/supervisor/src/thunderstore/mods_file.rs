//! The per-instance mod list as the *reconciler* reads it: `.grizzly/mods`, one
//! package per line, split by which machines each entry belongs on and paired
//! with the line number every later failure names.
//!
//! The line grammar itself lives in [`grizzly_control_api::mods`], because the
//! bot authors the same lines. What this module adds is the half only the
//! supervisor can supply — what a bad line *does* to a launch.
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

use grizzly_control_api::mods::{ClientLineProblem, InstallLine, ModsLine, classify};

use super::types::{PackageId, PackageRelease, PackageVersion, ParseError};

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
        match classify(raw) {
            ModsLine::Ignored => {}
            ModsLine::Install(Ok(entry)) => push_install(&mut list, entry, line_number)?,
            ModsLine::Install(Err(reason)) => {
                return Err(ModsFileError::Malformed {
                    line_number,
                    line: raw.trim().to_owned(),
                    reason,
                });
            }
            ModsLine::Client(Ok(release)) => push_client(&mut list, release, line_number),
            ModsLine::Client(Err(problem)) => {
                list.warnings
                    .push(client_warning(&problem, raw.trim(), line_number));
            }
        }
    }
    Ok(list)
}

/// Record a line the server installs, or fail the launch on a second listing of
/// the same package.
fn push_install(
    list: &mut ModsList,
    entry: InstallLine,
    line_number: usize,
) -> Result<(), ModsFileError> {
    if let Some(first_seen) = first_seen(list, &entry.id) {
        return Err(ModsFileError::Duplicate {
            line_number,
            id: entry.id,
            first_seen,
        });
    }
    list.install.push(ModsEntry {
        id: entry.id,
        pin: entry.pin,
        line_number,
    });
    Ok(())
}

/// Record a line that belongs on the players' machines, or warn and drop it.
fn push_client(list: &mut ModsList, release: PackageRelease, line_number: usize) {
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

/// Turn a rejected client line into the warning that names its actual fix. The
/// raw text is carried only by the unusable case — an unpinned entry is named by
/// its package, since re-reading the line is not what fixes it.
fn client_warning(problem: &ClientLineProblem, line: &str, line_number: usize) -> ModsFileWarning {
    match problem {
        ClientLineProblem::Unpinned(id) => ModsFileWarning::Unpinned {
            line_number,
            id: id.clone(),
        },
        ClientLineProblem::Unusable(reason) => ModsFileWarning::Unusable {
            line_number,
            line: line
                .strip_prefix(grizzly_control_api::mods::CLIENT_PREFIX)
                .unwrap_or(line)
                .trim()
                .to_owned(),
            reason: *reason,
        },
    }
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

#[cfg(test)]
#[path = "tests/mods_file.rs"]
mod tests;
