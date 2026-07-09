use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};

use crate::chat_watcher::ChatFormat;
use crate::rcon::RconDialect;

/// Default trigger a player types in chat to address the ops agent. Overridable
/// per game via `SUPERVISOR_CHAT_TRIGGER` if it collides with a game command.
const DEFAULT_CHAT_TRIGGER: &str = "@Gary";

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
/// How long the TCP readiness probe waits for the game to bind before giving up.
/// Generous: a first-boot `SteamCMD` download (Satisfactory pulls ~15 GB) plus
/// world generation can run for many minutes before the port opens.
const DEFAULT_READINESS_TIMEOUT_SECS: u64 = 600;
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
    /// How long the TCP readiness probe waits for the game to bind before giving
    /// up (and thus never signalling Agones `Ready`). Raise it for games with a
    /// long first-boot `SteamCMD` download. Unused on the log-pattern readiness path.
    pub readiness_timeout: Duration,
    /// How long to wait for a graceful child exit before SIGKILL.
    pub graceful_timeout: Duration,
    /// Sliding window for crash-rate escalation.
    pub crash_window: Duration,
    /// Crash count within `crash_window` that triggers escalation.
    pub crash_threshold: u32,
    /// Localhost port the game's RCON listens on, or `None` when the per-game
    /// template doesn't enable RCON. Its presence turns on the `/command` route.
    pub rcon_port: Option<u16>,
    /// Which console dialect the game speaks over RCON (reply framing + command
    /// vocabulary). Defaults to [`RconDialect::Source`]. Ignored when `rcon_port`
    /// is `None`.
    pub rcon_dialect: RconDialect,
    /// Child env var the minted RCON password is injected under. Ignored when
    /// `rcon_port` is `None`.
    pub rcon_password_env: String,
    /// Cap on the minted RCON password length (in characters), or `None` for the
    /// full-length default. Set for games that constrain their RCON/admin
    /// password — Palworld caps `ADMIN_PASSWORD` at 30 characters, and the
    /// supervisor must authenticate with exactly what the game accepts, so it
    /// truncates the minted password to this length on both sides. Ignored when
    /// `rcon_port` is `None`.
    pub rcon_password_max_len: Option<usize>,
    /// Substring that marks the game as ready in its log output, or `None` when
    /// the game uses the TCP connect probe instead. Set for UDP-only games
    /// (Valheim) that never open a TCP port the probe could reach: readiness is
    /// then signalled on the first captured line containing this string.
    pub ready_log_pattern: Option<String>,
    /// Path to a Palworld `PalWorldSettings.ini` the supervisor seeds with the
    /// RCON keys (enabled/port/`AdminPassword`) before each launch, or `None` for
    /// games that don't need it. Present only for Palworld, whose upstream image
    /// regenerates the whole ini from env on every boot: the image sets
    /// `DISABLE_GENERATE_SETTINGS=true` so the on-PVC ini is authoritative (the
    /// friend's/Gary's per-instance edits persist), and the supervisor owns just
    /// the RCON keys here so RCON still comes up on the minted password. Ignored
    /// when `rcon_port` is `None` — there's no password to seed without RCON.
    pub palworld_ini_path: Option<PathBuf>,
    /// Boot the control API but hold the game process down until a `/start` — set
    /// by the bot when it provisions a server to restore from an archive, so it
    /// can seed `/data` over the control API before the game ever launches (and
    /// generates a throwaway fresh world). Off by default: a normal server starts
    /// its game immediately.
    pub start_paused: bool,
    /// In-game chat watching, or `None` when the per-game template doesn't enable
    /// it (no `SUPERVISOR_CHAT_FORMAT`). Its presence spawns the watcher that
    /// forwards `@Gary` triggers to the bot.
    pub chat_watch: Option<ChatWatchConfig>,
}

/// Configuration for the in-game chat watcher, present only when the game enables
/// it. Built as a unit so the watcher gets every piece it needs or none at all —
/// a half-configured watcher (a format but nowhere to POST) is a misconfiguration
/// surfaced at parse time, not a silent no-op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatWatchConfig {
    /// How this game renders chat lines in its log stream.
    pub format: ChatFormat,
    /// The token a player types to address the agent (default [`DEFAULT_CHAT_TRIGGER`]).
    pub trigger: String,
    /// Full URL of the bot's agent endpoint to POST triggers to.
    pub agent_url: String,
    /// Shared bearer token authenticating the POST, or `None` when unset (the
    /// watcher warns; the endpoint is then NetworkPolicy-protected only).
    pub agent_token: Option<String>,
    /// The Agones `GameServer` name, sent so the bot maps the trigger back to a
    /// channel scope.
    pub server: String,
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
        let readiness_timeout = Duration::from_secs(
            optional_parse(lookup, "SUPERVISOR_READINESS_TIMEOUT_SECS")?
                .unwrap_or(DEFAULT_READINESS_TIMEOUT_SECS),
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
        let rcon_dialect = match optional(lookup, "SUPERVISOR_RCON_DIALECT") {
            Some(raw) => raw
                .parse::<RconDialect>()
                .context("failed to parse SUPERVISOR_RCON_DIALECT")?,
            None => RconDialect::Source,
        };
        let rcon_password_env = optional(lookup, "SUPERVISOR_RCON_PASSWORD_ENV")
            .unwrap_or_else(|| DEFAULT_RCON_PASSWORD_ENV.to_owned());
        let rcon_password_max_len = optional_parse(lookup, "SUPERVISOR_RCON_PASSWORD_MAX_LEN")?;
        let ready_log_pattern = optional(lookup, "SUPERVISOR_READY_LOG_PATTERN");
        let palworld_ini_path = optional(lookup, "SUPERVISOR_PALWORLD_INI").map(PathBuf::from);
        let start_paused = optional_flag(lookup, "SUPERVISOR_START_PAUSED");
        let chat_watch = parse_chat_watch(lookup)?;

        Ok(Self {
            child_command,
            game_port,
            control_port,
            sdk_base_url,
            data_dir,
            health_interval,
            readiness_timeout,
            graceful_timeout,
            crash_window,
            crash_threshold,
            rcon_port,
            rcon_dialect,
            rcon_password_env,
            rcon_password_max_len,
            ready_log_pattern,
            palworld_ini_path,
            start_paused,
            chat_watch,
        })
    }
}

/// Assemble the chat-watch config, or `None` when the game doesn't opt in
/// (`SUPERVISOR_CHAT_FORMAT` unset). Opting in but omitting the endpoint
/// (`SUPERVISOR_AGENT_URL`) or the server identity (`SUPERVISOR_GAMESERVER_NAME`)
/// is a hard error rather than a silent disable — a half-configured watcher would
/// swallow every `@Gary` with no signal.
fn parse_chat_watch(lookup: EnvLookup) -> Result<Option<ChatWatchConfig>> {
    let Some(format_raw) = optional(lookup, "SUPERVISOR_CHAT_FORMAT") else {
        return Ok(None);
    };
    let format = format_raw
        .parse::<ChatFormat>()
        .context("failed to parse SUPERVISOR_CHAT_FORMAT")?;
    let agent_url = optional(lookup, "SUPERVISOR_AGENT_URL").context(
        "SUPERVISOR_CHAT_FORMAT is set but SUPERVISOR_AGENT_URL is not; the watcher has \
         nowhere to send triggers",
    )?;
    let server = optional(lookup, "SUPERVISOR_GAMESERVER_NAME").context(
        "SUPERVISOR_CHAT_FORMAT is set but SUPERVISOR_GAMESERVER_NAME is not; the bot \
         can't map the trigger to a server",
    )?;
    let trigger = optional(lookup, "SUPERVISOR_CHAT_TRIGGER")
        .unwrap_or_else(|| DEFAULT_CHAT_TRIGGER.to_owned());
    let agent_token = optional(lookup, "SUPERVISOR_AGENT_TOKEN");
    Ok(Some(ChatWatchConfig {
        format,
        trigger,
        agent_url,
        agent_token,
        server,
    }))
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
