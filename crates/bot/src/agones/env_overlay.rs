//! Which settings an instance of a game may override for itself.
//!
//! [ADR-012](../../docs/decisions/012-per-instance-env-overlay.md) gave the
//! platform a per-instance environment tier: a reserved dotenv file on the PVC,
//! filtered through an allowlist the *catalog* declares, layered into the game
//! process at each launch. The supervisor enforces it. This module is the bot's
//! side of the same two variables — it reads them so Gary can be told which
//! settings a game actually exposes, rather than exploring for a file that may
//! not exist.
//!
//! That matters most on the games where it is the *only* path. Most of the
//! catalog keeps its settings in the game's own config files, which Gary finds
//! by browsing; Valheim's join password is an environment variable and nothing
//! else, so with no overlay declared there is no answer to find and the search
//! cannot terminate.
//!
//! Both variables are required together, matching the supervisor's own rule: an
//! allowlist with no file has nothing to read, and a file with no allowlist
//! admits nothing. Either alone is a half-finished catalog edit and reads here
//! as "no overlay".

use anyhow::Result;
use tracing::warn;

use super::template::AnnotatedTemplate;

/// Data-root-relative overlay file an instance keeps its settings in.
const CHILD_ENV_FILE: &str = "SUPERVISOR_CHILD_ENV_FILE";
/// Comma-separated keys the supervisor will carry out of that file.
const CHILD_ENV_ALLOWLIST: &str = "SUPERVISOR_CHILD_ENV_ALLOWLIST";

/// The per-instance settings surface one game exposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvOverlay {
    /// Where the file sits, relative to the data root Gary's file tools are
    /// scoped to — so it is already the path he would pass to `read_file`.
    pub(crate) file: String,
    /// The keys that do anything in it, in the order the catalog lists them.
    /// Never empty: an overlay with no keys is not an overlay.
    pub(crate) keys: Vec<String>,
}

impl EnvOverlay {
    /// The allowlist as prose for a prompt: `A, B and C`.
    ///
    /// Rendered from the parsed catalog rather than restated in prompt text, so
    /// widening the allowlist cannot leave Gary describing the old one.
    pub(crate) fn keys_sentence(&self) -> String {
        match self.keys.split_last() {
            // The vector is never empty, so this arm is the one-key case.
            None | Some((_, [])) => self.keys.join(""),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    }
}

/// Read a game's settings overlay off its raw `GameServer` template.
///
/// # Errors
///
/// Returns an error if the template is not parseable YAML.
pub(crate) fn parse_env_overlay_from_template(gameserver_yaml: &str) -> Result<Option<EnvOverlay>> {
    let template = AnnotatedTemplate::parse(gameserver_yaml)?;
    let (Some(file), Some(allowlist)) = (
        template.env_value(CHILD_ENV_FILE),
        template.env_value(CHILD_ENV_ALLOWLIST),
    ) else {
        return Ok(None);
    };
    let keys: Vec<String> = allowlist
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
        .collect();
    if keys.is_empty() {
        return Ok(None);
    }
    Ok(Some(EnvOverlay {
        file: file.to_owned(),
        keys,
    }))
}

/// [`parse_env_overlay_from_template`], degraded to "no overlay" with a warning
/// — the same leniency the catalog loader applies to every other per-game fact,
/// and for the same reason: one game's malformed template must not stop the bot
/// from serving the rest.
pub(crate) fn env_overlay_or_warn(id: &str, gameserver_yaml: &str) -> Option<EnvOverlay> {
    match parse_env_overlay_from_template(gameserver_yaml) {
        Ok(overlay) => overlay,
        Err(err) => {
            warn!(
                error = ?err,
                game = %id,
                "catalog: unreadable GameServer template; this game's settings overlay is unknown"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/env_overlay.rs"]
mod tests;
