use std::fmt;

use grizzly_control_api::mods::{ClientLineProblem, InstallLine, ModsLine, classify};

use super::types::{PackageId, PackageRelease, PackageVersion, ParseError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModsEntry {
    pub id: PackageId,
    pub pin: Option<PackageVersion>,
    pub line_number: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientEntry {
    pub release: PackageRelease,
    pub line_number: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModsList {
    pub install: Vec<ModsEntry>,
    pub client_only: Vec<ClientEntry>,
    pub warnings: Vec<ModsFileWarning>,
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModsFileWarning {
    Unusable {
        line_number: usize,
        line: String,
        reason: ParseError,
    },
    Unpinned {
        line_number: usize,
        id: PackageId,
    },
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
