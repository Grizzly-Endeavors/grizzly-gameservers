use anyhow::Result;
use tracing::warn;

use super::template::AnnotatedTemplate;

const CHILD_ENV_FILE: &str = "SUPERVISOR_CHILD_ENV_FILE";
const CHILD_ENV_ALLOWLIST: &str = "SUPERVISOR_CHILD_ENV_ALLOWLIST";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EnvOverlay {
    pub(crate) file: String,
    pub(crate) keys: Vec<String>,
}

impl EnvOverlay {
    pub(crate) fn keys_sentence(&self) -> String {
        match self.keys.split_last() {
            None | Some((_, [])) => self.keys.join(""),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        }
    }
}

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
