use std::ffi::OsString;

use anyhow::{Context, Result, anyhow, bail};

/// Closure that resolves an environment variable to its raw value, mirroring
/// `std::env::var_os`. Injected so config parsing is testable without the
/// `unsafe` `set_var`/`remove_var` of Rust 2024.
pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

const DEFAULT_NAMESPACE: &str = "game-servers";
const DEFAULT_DOMAIN: &str = "gameservers.bearflinn.com";

/// Runtime configuration for the bot, sourced from the process environment.
#[derive(Clone, Debug)]
pub struct BotConfig {
    pub(crate) token: String,
    pub(crate) guild_id: u64,
    pub(crate) namespace: String,
    pub(crate) domain: String,
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

        Ok(Self {
            token,
            guild_id,
            namespace,
            domain,
        })
    }
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
