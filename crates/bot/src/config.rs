use std::ffi::OsString;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};

/// Closure that resolves an environment variable to its raw value, mirroring
/// `std::env::var_os`. Injected so config parsing is testable without the
/// `unsafe` `set_var`/`remove_var` of Rust 2024.
pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

const DEFAULT_NAMESPACE: &str = "game-servers";
const DEFAULT_DOMAIN: &str = "gameservers.bearflinn.com";
/// Where the per-game catalog is baked into the container image (see Dockerfile).
const DEFAULT_CATALOG_DIR: &str = "/usr/local/share/grizzly-gameservers/games";

/// Runtime configuration for the bot, sourced from the process environment.
#[derive(Clone, Debug)]
pub struct BotConfig {
    pub(crate) token: String,
    pub(crate) guild_id: u64,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) catalog_dir: PathBuf,
    /// Discord role whose members may run the mutating commands.
    pub(crate) admin_role_id: Option<u64>,
    /// Explicit user-id allowlist for the mutating commands.
    pub(crate) admin_user_ids: Vec<u64>,
}

impl BotConfig {
    /// Build configuration from the real process environment.
    ///
    /// # Errors
    ///
    /// Returns an error if `DISCORD_BOT_TOKEN` or `DISCORD_GUILD_ID` is unset,
    /// non-UTF-8, or (for the guild id) not a non-zero integer.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&|key| std::env::var_os(key))
    }

    pub(crate) fn from_env_with(lookup: EnvLookup) -> Result<Self> {
        let token = required(lookup, "DISCORD_BOT_TOKEN")?;
        let guild_raw = required(lookup, "DISCORD_GUILD_ID")?;
        let guild_id = guild_raw.parse::<u64>().with_context(|| {
            format!("DISCORD_GUILD_ID must be a positive integer, got {guild_raw:?}")
        })?;
        if guild_id == 0 {
            bail!("DISCORD_GUILD_ID must be non-zero");
        }

        let namespace = optional(lookup, "GAMESERVERS_NAMESPACE")
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_owned());
        let domain =
            optional(lookup, "GAMESERVERS_DOMAIN").unwrap_or_else(|| DEFAULT_DOMAIN.to_owned());
        let catalog_dir = optional(lookup, "GAMESERVERS_CATALOG_DIR")
            .map_or_else(|| PathBuf::from(DEFAULT_CATALOG_DIR), PathBuf::from);
        let admin_role_id = optional_u64(lookup, "GAMESERVERS_ADMIN_ROLE_ID")?;
        let admin_user_ids =
            parse_user_ids(optional(lookup, "GAMESERVERS_ADMIN_USER_IDS").as_deref())?;

        Ok(Self {
            token,
            guild_id,
            namespace,
            domain,
            catalog_dir,
            admin_role_id,
            admin_user_ids,
        })
    }
}

fn optional_u64(lookup: EnvLookup, key: &str) -> Result<Option<u64>> {
    match optional(lookup, key) {
        Some(raw) => {
            let value = raw
                .parse::<u64>()
                .with_context(|| format!("{key} must be a positive integer, got {raw:?}"))?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

/// Parse a comma-separated list of Discord user ids. Blank entries are ignored
/// so trailing commas and whitespace are tolerated.
fn parse_user_ids(raw: Option<&str>) -> Result<Vec<u64>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry.parse::<u64>().with_context(|| {
                format!("GAMESERVERS_ADMIN_USER_IDS contains a non-integer: {entry:?}")
            })
        })
        .collect()
}

fn required(lookup: EnvLookup, key: &str) -> Result<String> {
    let raw = lookup(key).with_context(|| format!("{key} is required but not set"))?;
    raw.into_string()
        .map_err(|bad| anyhow!("{key} is not valid UTF-8: {}", bad.display()))
}

fn optional(lookup: EnvLookup, key: &str) -> Option<String> {
    lookup(key).and_then(|raw| raw.into_string().ok())
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
