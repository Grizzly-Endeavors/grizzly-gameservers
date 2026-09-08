//! The per-instance mod list: `.grizzly/mods`, one package per line.
//!
//! Grammar: `Namespace-Name` (latest at resolve time) or `Namespace-Name-1.2.3`
//! (pinned). Blank lines and `#` comments are ignored. Trailing and leading
//! whitespace is trimmed.
//!
//! Unlike the env overlay next door, a bad line here is a **hard error** that
//! fails the launch. That asymmetry is deliberate and load-bearing: the overlay
//! degrades to "no overrides", which is a server that still boots, while a typo
//! in this file would otherwise degrade to "that mod isn't installed" — a
//! silent, wrong, and durable answer to a friend who asked for it. Failing loud
//! routes the mistake into the bot's snapshot-restore path, which puts the file
//! back the way it was and restarts.

use std::fmt;
use std::str::FromStr;

use super::types::{PackageId, PackageVersion, ParseError};

/// One entry in the list: a package, optionally pinned to an exact version, and
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

/// Parse the mod list.
///
/// # Errors
///
/// Returns the first [`ModsFileError`] encountered: a line that is neither a
/// package nor a pinned package, or a package listed twice. Both stop the launch
/// rather than being skipped — see the module docs.
pub fn parse(text: &str) -> Result<Vec<ModsEntry>, ModsFileError> {
    let mut entries: Vec<ModsEntry> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line_number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = parse_line(line, line_number)?;
        if let Some(first) = entries.iter().find(|existing| existing.id == entry.id) {
            return Err(ModsFileError::Duplicate {
                line_number,
                id: entry.id,
                first_seen: first.line_number,
            });
        }
        entries.push(entry);
    }
    Ok(entries)
}

/// One non-blank, non-comment line. Tries the pinned shape first: a bare
/// `Namespace-Name` never parses as a release (a release needs a numeric third
/// field), so the order can't misread one as the other.
fn parse_line(line: &str, line_number: usize) -> Result<ModsEntry, ModsFileError> {
    if let Ok(release) = line.parse::<super::types::PackageRelease>() {
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
