use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::config::SupervisorConfig;

const RESERVED_KEY_PREFIX: &str = "SUPERVISOR_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlaySkip {
    Malformed { line_number: usize },
    Duplicate { key: String },
    NotAllowlisted { key: String },
    Reserved { key: String, allowlisted: bool },
    PlatformOwned { key: String, allowlisted: bool },
}

impl OverlaySkip {
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

    fn key(&self) -> Option<&str> {
        match self {
            Self::Malformed { .. } => None,
            Self::Duplicate { key }
            | Self::NotAllowlisted { key }
            | Self::Reserved { key, .. }
            | Self::PlatformOwned { key, .. } => Some(key),
        }
    }

    const fn is_catalog_error(&self) -> bool {
        match self {
            Self::Reserved { allowlisted, .. } | Self::PlatformOwned { allowlisted, .. } => {
                *allowlisted
            }
            Self::Malformed { .. } | Self::Duplicate { .. } | Self::NotAllowlisted { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    pub applied: Vec<(String, String)>,
    pub skipped: Vec<OverlaySkip>,
}

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

pub fn seed(root: &Path, rel: &str) -> Result<PathBuf> {
    crate::fs::seed_empty_file(root, rel)
        .map_err(|err| anyhow::anyhow!("{err}"))
        .with_context(|| format!("failed to seed the child env overlay {rel}"))
}

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
