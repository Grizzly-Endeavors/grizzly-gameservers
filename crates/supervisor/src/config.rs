use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

/// Closure that resolves an environment variable to its raw value, mirroring
/// `std::env::var_os`. Injected so config parsing is testable without the
/// `unsafe` `set_var`/`remove_var` of Rust 2024.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// The itzg entrypoint the supervisor wraps as its child process.
const DEFAULT_CHILD_CMD: &str = "/start";
const DEFAULT_GAME_PORT: u16 = 25565;
/// Not in the 7000–7010 `NodePort` band and not 9358 (the Agones SDK).
const DEFAULT_CONTROL_PORT: u16 = 9359;
/// Where the auto-injected Agones SDK sidecar serves its REST API.
const DEFAULT_SDK_BASE_URL: &str = "http://127.0.0.1:9358";
/// The instance PVC mount the agent's file operations are confined to — matches
/// the Minecraft `volumeMounts.mountPath`. Games whose PVC mounts elsewhere
/// override it via `SUPERVISOR_DATA_DIR`.
const DEFAULT_DATA_DIR: &str = "/data";
/// 5s gives a 3× margin against the catalog health budget
/// (periodSeconds 15 × failureThreshold 5 = 75s).
const DEFAULT_HEALTH_INTERVAL_SECS: u64 = 5;
/// Generous relative to a Minecraft world-save before we SIGKILL.
const DEFAULT_GRACEFUL_TIMEOUT_SECS: u64 = 90;
/// Sliding window over which repeated crashes count toward escalation.
const DEFAULT_CRASH_WINDOW_SECS: u64 = 300;
/// Crashes within the window before the supervisor stops the heartbeat and lets
/// Agones recreate the pod.
const DEFAULT_CRASH_THRESHOLD: u32 = 5;
/// Child env var the minted RCON password is injected under. Matches itzg's
/// `RCON_PASSWORD`; a Source game overrides it to whatever its entrypoint reads.
const DEFAULT_RCON_PASSWORD_ENV: &str = "RCON_PASSWORD";

/// Runtime configuration for the supervisor, sourced from the process
/// environment. Every knob has a default so the container can run with no env.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    /// Command launched as the supervised child (the game server entrypoint).
    pub child_command: String,
    /// TCP port the game accepts connections on, probed for readiness.
    pub game_port: u16,
    /// Port the HTTP control API binds (pod-internal; never NodePort-exposed).
    pub control_port: u16,
    /// Base URL of the Agones SDK sidecar's REST API.
    pub sdk_base_url: String,
    /// Root the agent's file operations are confined to (the instance PVC mount).
    pub data_dir: PathBuf,
    /// How often to ping the SDK `/health` endpoint.
    pub health_interval: Duration,
    /// How long to wait for a graceful child exit before SIGKILL.
    pub graceful_timeout: Duration,
    /// Sliding window for crash-rate escalation.
    pub crash_window: Duration,
    /// Crash count within `crash_window` that triggers escalation.
    pub crash_threshold: u32,
    /// Localhost port the game's RCON listens on, or `None` when the per-game
    /// template doesn't enable RCON. Its presence turns on the `/command` route.
    pub rcon_port: Option<u16>,
    /// Whether the RCON client runs in Minecraft-quirks mode (Minecraft's RCON
    /// diverges from the base Source protocol). Ignored when `rcon_port` is `None`.
    pub rcon_minecraft: bool,
    /// Child env var the minted RCON password is injected under. Ignored when
    /// `rcon_port` is `None`.
    pub rcon_password_env: String,
    /// Boot the control API but hold the game process down until a `/start` — set
    /// by the bot when it provisions a server to restore from an archive, so it
    /// can seed `/data` over the control API before the game ever launches (and
    /// generates a throwaway fresh world). Off by default: a normal server starts
    /// its game immediately.
    pub start_paused: bool,
}

impl SupervisorConfig {
    /// Build configuration from the real process environment.
    ///
    /// # Errors
    ///
    /// Returns an error if a set variable is non-UTF-8 or fails to parse as its
    /// expected numeric type.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&|key| std::env::var_os(key))
    }

    /// Build configuration from an injected environment lookup.
    ///
    /// # Errors
    ///
    /// Returns an error if a set variable is non-UTF-8 or fails to parse as its
    /// expected numeric type.
    pub fn from_env_with(lookup: EnvLookup) -> Result<Self> {
        let child_command = optional(lookup, "SUPERVISOR_CHILD_CMD")
            .unwrap_or_else(|| DEFAULT_CHILD_CMD.to_owned());
        let game_port =
            optional_parse(lookup, "SUPERVISOR_GAME_PORT")?.unwrap_or(DEFAULT_GAME_PORT);
        let control_port =
            optional_parse(lookup, "SUPERVISOR_CONTROL_PORT")?.unwrap_or(DEFAULT_CONTROL_PORT);
        let sdk_base_url =
            optional(lookup, "AGONES_SDK_HTTP").unwrap_or_else(|| DEFAULT_SDK_BASE_URL.to_owned());
        let data_dir = PathBuf::from(
            optional(lookup, "SUPERVISOR_DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.to_owned()),
        );
        let health_interval = Duration::from_secs(
            optional_parse(lookup, "SUPERVISOR_HEALTH_INTERVAL_SECS")?
                .unwrap_or(DEFAULT_HEALTH_INTERVAL_SECS),
        );
        let graceful_timeout = Duration::from_secs(
            optional_parse(lookup, "SUPERVISOR_GRACEFUL_TIMEOUT_SECS")?
                .unwrap_or(DEFAULT_GRACEFUL_TIMEOUT_SECS),
        );
        let crash_window = Duration::from_secs(
            optional_parse(lookup, "SUPERVISOR_CRASH_WINDOW_SECS")?
                .unwrap_or(DEFAULT_CRASH_WINDOW_SECS),
        );
        let crash_threshold = optional_parse(lookup, "SUPERVISOR_CRASH_THRESHOLD")?
            .unwrap_or(DEFAULT_CRASH_THRESHOLD);
        let rcon_port = optional_parse(lookup, "SUPERVISOR_RCON_PORT")?;
        let rcon_minecraft = optional_flag(lookup, "SUPERVISOR_RCON_MINECRAFT");
        let rcon_password_env = optional(lookup, "SUPERVISOR_RCON_PASSWORD_ENV")
            .unwrap_or_else(|| DEFAULT_RCON_PASSWORD_ENV.to_owned());
        let start_paused = optional_flag(lookup, "SUPERVISOR_START_PAUSED");

        Ok(Self {
            child_command,
            game_port,
            control_port,
            sdk_base_url,
            data_dir,
            health_interval,
            graceful_timeout,
            crash_window,
            crash_threshold,
            rcon_port,
            rcon_minecraft,
            rcon_password_env,
            start_paused,
        })
    }
}

fn optional(lookup: EnvLookup, key: &str) -> Option<String> {
    lookup(key).and_then(|raw| raw.into_string().ok())
}

/// Interpret an optional flag variable as a boolean, accepting the common truthy
/// spellings case-insensitively. Absent or unrecognized values are `false`.
fn optional_flag(lookup: EnvLookup, key: &str) -> bool {
    optional(lookup, key).is_some_and(|raw| {
        matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Parse an optional variable into any `FromStr` numeric type, surfacing both
/// non-UTF-8 and parse failures with the offending key and value.
fn optional_parse<T>(lookup: EnvLookup, key: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match lookup(key) {
        Some(raw) => {
            let text = raw
                .into_string()
                .map_err(|bad| anyhow!("{key} is not valid UTF-8: {}", bad.display()))?;
            let value = text
                .parse::<T>()
                .map_err(|err| anyhow!("{key} is invalid ({err}), got {text:?}"))
                .with_context(|| format!("failed to parse {key}"))?;
            Ok(Some(value))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
