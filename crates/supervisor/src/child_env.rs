//! Per-instance child-environment overlay.
//!
//! The platform's per-instance config tier for environment variables. Before
//! each child launch the supervisor reads a reserved text file from the data
//! volume and injects an allowlisted subset of its keys into the child process
//! environment, so an instance can be tuned without a pod recreation or a
//! catalog change. The Minecraft catalog uses it for itzg's mod-management vars
//! (`TYPE`, `MODRINTH_PROJECTS`, …), which itzg re-reads on every start.
//!
//! The mechanism is game-agnostic and off unless the game's catalog entry names
//! the file; which keys an instance may override is per-game knowledge, so the
//! allowlist ships in the catalog rather than being hardcoded here.
//!
//! Two properties are load-bearing and deliberate:
//!
//! - **Seeding an empty file.** [`fs::write_file`](crate::fs::write_file) only
//!   snapshots a file that already existed, and the bot's auto-rollback loop only
//!   arms itself for writes that took a snapshot. Because the overlay file always
//!   exists before anyone edits it, even the very first change is an overwrite —
//!   snapshotted, verified, and rolled back like every later one.
//! - **Never failing a launch.** Every failure here degrades to a warning and an
//!   inert overlay. A broken overlay must leave a server bootable, since repairing
//!   the file is exactly what the operator loop needs the server up for.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::config::SupervisorConfig;

/// Key prefix the supervisor reserves for its own configuration. An overlay may
/// never set one: those vars are read from the pod template at supervisor
/// startup, so honoring them would let an instance rewrite its own guardrails
/// (including the allowlist that bounds it).
const RESERVED_KEY_PREFIX: &str = "SUPERVISOR_";

/// Why a line or key from the overlay file was not applied. Carried out of the
/// pure [`resolve`] so the caller does the logging and the parsing stays testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlaySkip {
    /// A non-blank, non-comment line with no `=`, or with an empty key.
    Malformed { line_number: usize },
    /// An earlier occurrence of a key that appears again later in the file.
    Duplicate { key: String },
    /// The key is not in the catalog-declared allowlist.
    NotAllowlisted { key: String },
    /// The key is [`RESERVED_KEY_PREFIX`]-prefixed. Denied regardless of the
    /// allowlist; `allowlisted` marks the catalog misconfiguration where it was
    /// listed anyway.
    Reserved { key: String, allowlisted: bool },
    /// The key is the instance's resolved RCON password variable, which the
    /// supervisor mints and owns. Same allowlist-independent denial.
    PlatformOwned { key: String, allowlisted: bool },
}

impl OverlaySkip {
    /// Plain-language reason this key was dropped, for the warning log.
    fn reason(&self) -> &'static str {
        match self {
            Self::Malformed { .. } => "line is not KEY=VALUE",
            Self::Duplicate { .. } => "key set more than once; the last occurrence wins",
            Self::NotAllowlisted { .. } => {
                "key is not in this game's allowlist (keys are case-sensitive)"
            }
            Self::Reserved { .. } => "keys starting with SUPERVISOR_ are reserved by the platform",
            Self::PlatformOwned { .. } => "the rcon password is minted by the supervisor",
        }
    }

    /// The offending key, or `None` for a line that had no usable key.
    fn key(&self) -> Option<&str> {
        match self {
            Self::Malformed { .. } => None,
            Self::Duplicate { key }
            | Self::NotAllowlisted { key }
            | Self::Reserved { key, .. }
            | Self::PlatformOwned { key, .. } => Some(key),
        }
    }

    /// Whether this skip means the catalog itself is wrong (a denied key was
    /// allowlisted) rather than the instance's file being wrong. The distinction
    /// matters because only the former needs a git change to fix.
    const fn is_catalog_error(&self) -> bool {
        match self {
            Self::Reserved { allowlisted, .. } | Self::PlatformOwned { allowlisted, .. } => {
                *allowlisted
            }
            Self::Malformed { .. } | Self::Duplicate { .. } | Self::NotAllowlisted { .. } => false,
        }
    }
}

/// The outcome of reading one overlay file: what to inject, and what was dropped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    /// Key/value pairs to set on the child, in file order.
    pub applied: Vec<(String, String)>,
    /// Everything that was dropped, in the order it was encountered.
    pub skipped: Vec<OverlaySkip>,
}

/// Parse `text` as the overlay's dotenv subset and filter it down to the keys an
/// instance is allowed to set.
///
/// Format: one `KEY=VALUE` per line; blank lines and `#`-prefixed lines ignored;
/// the key is everything before the first `=` and the value everything after,
/// both trimmed of surrounding whitespace. There is no quoting or escaping — a
/// quoted value keeps its quotes. Keys are matched case-sensitively, so a
/// mis-cased key simply misses the allowlist. When a key repeats, the last
/// occurrence wins.
///
/// `rcon_password_env` is the instance's *resolved* password variable (games
/// rename it), not a fixed literal.
#[must_use]
pub fn resolve(text: &str, allowlist: &[String], rcon_password_env: &str) -> Resolution {
    let mut resolution = Resolution::default();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = trimmed.split_once('=') else {
            resolution.skipped.push(OverlaySkip::Malformed {
                line_number: index + 1,
            });
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() {
            resolution.skipped.push(OverlaySkip::Malformed {
                line_number: index + 1,
            });
            continue;
        }
        let allowlisted = allowlist.iter().any(|allowed| allowed == key);
        // Denies are checked before the allowlist so a catalog that mistakenly
        // lists a reserved key is reported as such rather than silently honored.
        if key.starts_with(RESERVED_KEY_PREFIX) {
            resolution.skipped.push(OverlaySkip::Reserved {
                key: key.to_owned(),
                allowlisted,
            });
            continue;
        }
        if key == rcon_password_env {
            resolution.skipped.push(OverlaySkip::PlatformOwned {
                key: key.to_owned(),
                allowlisted,
            });
            continue;
        }
        if !allowlisted {
            resolution.skipped.push(OverlaySkip::NotAllowlisted {
                key: key.to_owned(),
            });
            continue;
        }
        if let Some(position) = resolution
            .applied
            .iter()
            .position(|(existing, _)| existing == key)
        {
            resolution.applied.remove(position);
            resolution.skipped.push(OverlaySkip::Duplicate {
                key: key.to_owned(),
            });
        }
        resolution
            .applied
            .push((key.to_owned(), raw_value.trim().to_owned()));
    }
    resolution
}

/// Create the overlay file and its parent directory if absent, so the file
/// always exists before anyone edits it (see the module docs for why that is
/// load-bearing). Delegates to [`fs::seed_empty_file`](crate::fs::seed_empty_file),
/// which the mod list next door uses for the same reason.
///
/// # Errors
///
/// Returns an error if `rel` escapes the data root, the parent directory can't
/// be created, or the file can't be created.
pub fn seed(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::fs::seed_empty_file(root, rel)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .with_context(|| format!("failed to seed the child env overlay {rel}"))
}

/// Seed the overlay file if the game opted in, then read and filter it into the
/// key/value pairs to set on the next child.
///
/// Infallible by design: a game that didn't opt in, a seeding failure, an
/// unreadable file, and a file full of rejected keys all yield an empty overlay
/// and a warning rather than blocking the launch.
///
/// Logs name keys only — never values — so a mis-allowlisted key can't leak its
/// contents into the pod's log stream.
#[must_use]
pub fn prepare(cfg: &SupervisorConfig) -> Vec<(String, String)> {
    let Some(overlay) = cfg.child_env.as_ref() else {
        return Vec::new();
    };
    let file = overlay.file.as_str();
    let path = match seed(&cfg.data_dir, file) {
        Ok(path) => path,
        Err(err) => {
            warn!(
                error = ?err,
                file,
                "could not seed the per-instance env overlay; it is inert for this launch"
            );
            return Vec::new();
        }
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        // Seeding just guaranteed the file exists, so NotFound here means it was
        // removed between the two calls. Treat it as empty, same as the contract
        // says a missing file is.
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(err) => {
            warn!(
                error = %err,
                file,
                "could not read the per-instance env overlay; it is inert for this launch"
            );
            return Vec::new();
        }
    };

    let resolution = resolve(&text, &overlay.allowlist, &cfg.rcon_password_env);
    for skip in &resolution.skipped {
        let reason = skip.reason();
        match skip.key() {
            Some(key) if skip.is_catalog_error() => warn!(
                file,
                key, reason, "the game's env allowlist names a key the platform reserves"
            ),
            Some(key) => warn!(file, key, reason, "skipped a key in the env overlay"),
            None => warn!(file, reason, "skipped a line in the env overlay"),
        }
    }
    if !resolution.applied.is_empty() {
        let keys = resolution
            .applied
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        info!(file, keys = %keys, "applying per-instance env overlay");
    }
    resolution.applied
}

#[cfg(test)]
#[path = "tests/child_env.rs"]
mod tests;
