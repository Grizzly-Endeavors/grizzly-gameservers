use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tracing::warn;

use crate::autoupdate::AutoUpdatePolicy;
use crate::chat_watcher::ChatFormat;
use crate::occupancy::{OccupancyApi, OccupancyEndpoint};
use crate::rcon::RconDialect;
use crate::thunderstore::archive::PackageLayout;
use crate::thunderstore::types::PackageId;

const DEFAULT_CHAT_TRIGGER: &str = "@Gary";

pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

const DEFAULT_CHILD_CMD: &str = "/start";
const DEFAULT_GAME_PORT: u16 = 25565;
const DEFAULT_CONTROL_PORT: u16 = grizzly_control_api::CONTROL_PORT;
const DEFAULT_SDK_BASE_URL: &str = "http://127.0.0.1:9358";
const DEFAULT_DATA_DIR: &str = "/data";
const DEFAULT_HEALTH_INTERVAL_SECS: u64 = 5;
const DEFAULT_READINESS_TIMEOUT_SECS: u64 = 600;
const DEFAULT_GRACEFUL_TIMEOUT_SECS: u64 = grizzly_control_api::DEFAULT_GRACEFUL_TIMEOUT_SECS;
const DEFAULT_CRASH_WINDOW_SECS: u64 = 300;
const DEFAULT_CRASH_THRESHOLD: u32 = 5;
const DEFAULT_RCON_PASSWORD_ENV: &str = "RCON_PASSWORD";
const DEFAULT_AUTO_UPDATE_POLL_SECS: u64 = 120;
const DEFAULT_AUTO_UPDATE_EMPTY_GRACE_SECS: u64 = 300;
const DEFAULT_AUTO_UPDATE_INTERVAL_SECS: u64 = 86_400;
const DEFAULT_AUTO_UPDATE_MAX_UPTIME_SECS: u64 = 259_200;
const DEFAULT_THUNDERSTORE_DIR: &str = grizzly_control_api::mods::DEFAULT_WORK_DIR;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub child_command: String,
    pub game_port: u16,
    pub control_port: u16,
    pub sdk_base_url: String,
    pub data_dir: PathBuf,
    pub health_interval: Duration,
    pub readiness_timeout: Duration,
    pub graceful_timeout: Duration,
    /// Shared secret the bot must present on the control API, or `None` to accept
    /// any caller the network leash already allowed through. Absent by default so
    /// an image or deployment without the secret keeps working rather than locking
    /// the bot out of every server at once.
    pub control_token: Option<String>,
    pub crash_window: Duration,
    pub crash_threshold: u32,
    /// Where the game's player count comes from when it isn't RCON, or `None` for
    /// games that report it over RCON (or not at all). Its presence selects the
    /// alternate source, which is the only way Satisfactory, Terraria and
    /// Enshrouded get a player count — none has a console, and without a count
    /// none can join update-on-empty.
    pub occupancy: Option<OccupancyEndpoint>,
    pub rcon_port: Option<u16>,
    pub rcon_dialect: RconDialect,
    pub rcon_password_env: String,
    /// Cap on the minted RCON password length (in characters), or `None` for the
    /// full-length default. Set for games that constrain their RCON/admin
    /// password — Palworld caps `ADMIN_PASSWORD` at 30 characters, and the
    /// supervisor must authenticate with exactly what the game accepts, so it
    /// truncates the minted password to this length on both sides. Ignored when
    /// `rcon_port` is `None`.
    pub rcon_password_max_len: Option<usize>,
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
    pub start_paused: bool,
    pub chat_watch: Option<ChatWatchConfig>,
    pub child_env: Option<ChildEnvOverlay>,
    pub thunderstore: Option<ThunderstoreMods>,
    pub auto_update_enabled: bool,
    pub auto_update_poll_interval: Duration,
    pub auto_update_policy: AutoUpdatePolicy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildEnvOverlay {
    pub file: String,
    pub allowlist: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThunderstoreMods {
    pub file: String,
    /// The Thunderstore community this game's mods come from. Packages never
    /// cross between communities, so this names which catalog the mod list is
    /// written against; it is logged at startup so a mismatch between the image
    /// and the bot's catalog annotation is visible in one place.
    pub community: String,
    pub layout: PackageLayout,
    pub provided: Vec<PackageId>,
    pub work_dir: String,
    pub config_dir: Option<String>,
    pub config_keep: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatWatchConfig {
    pub format: ChatFormat,
    pub trigger: String,
    pub agent_url: String,
    pub agent_token: Option<String>,
    pub server: String,
}

impl SupervisorConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&|key| std::env::var_os(key))
    }

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
        let control_token = optional(lookup, "SUPERVISOR_CONTROL_TOKEN");
        let occupancy = parse_occupancy(lookup)?;
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
        let child_env = parse_child_env(lookup);
        let thunderstore = parse_thunderstore(lookup)?;
        let auto_update_enabled = optional_flag_or(lookup, "SUPERVISOR_AUTO_UPDATE", true);
        let auto_update_poll_interval = Duration::from_secs(
            optional_parse(lookup, "SUPERVISOR_AUTO_UPDATE_POLL_SECS")?
                .unwrap_or(DEFAULT_AUTO_UPDATE_POLL_SECS),
        );
        let auto_update_policy = AutoUpdatePolicy {
            empty_grace: Duration::from_secs(
                optional_parse(lookup, "SUPERVISOR_AUTO_UPDATE_EMPTY_GRACE_SECS")?
                    .unwrap_or(DEFAULT_AUTO_UPDATE_EMPTY_GRACE_SECS),
            ),
            update_interval: Duration::from_secs(
                optional_parse(lookup, "SUPERVISOR_AUTO_UPDATE_INTERVAL_SECS")?
                    .unwrap_or(DEFAULT_AUTO_UPDATE_INTERVAL_SECS),
            ),
            max_uptime: Duration::from_secs(
                optional_parse(lookup, "SUPERVISOR_AUTO_UPDATE_MAX_UPTIME_SECS")?
                    .unwrap_or(DEFAULT_AUTO_UPDATE_MAX_UPTIME_SECS),
            ),
        };

        warn_if_rcon_settings_without_port(lookup, rcon_port);
        warn_if_child_env_half_configured(lookup, child_env.as_ref());

        Ok(Self {
            child_command,
            game_port,
            control_port,
            sdk_base_url,
            data_dir,
            health_interval,
            readiness_timeout,
            graceful_timeout,
            control_token,
            crash_window,
            crash_threshold,
            occupancy,
            rcon_port,
            rcon_dialect,
            rcon_password_env,
            rcon_password_max_len,
            ready_log_pattern,
            palworld_ini_path,
            start_paused,
            chat_watch,
            child_env,
            thunderstore,
            auto_update_enabled,
            auto_update_poll_interval,
            auto_update_policy,
        })
    }
}

fn parse_occupancy(lookup: EnvLookup) -> Result<Option<OccupancyEndpoint>> {
    let api = match optional(lookup, "SUPERVISOR_OCCUPANCY_API") {
        None => None,
        Some(raw) => Some(OccupancyApi::parse(&raw).ok_or_else(|| {
            anyhow!(
                "SUPERVISOR_OCCUPANCY_API must be 'satisfactory', 'tshock' or 'a2s', got {raw:?}"
            )
        })?),
    };
    let url = optional(lookup, "SUPERVISOR_OCCUPANCY_API_URL");
    let a2s_port = optional_parse::<u16>(lookup, "SUPERVISOR_A2S_PORT")?;

    match (api, url, a2s_port) {
        (Some(OccupancyApi::A2s), None, Some(port)) => Ok(Some(OccupancyEndpoint::A2s(port))),
        (Some(OccupancyApi::A2s), None, None) => {
            bail!("SUPERVISOR_OCCUPANCY_API is 'a2s' but SUPERVISOR_A2S_PORT is not set")
        }
        (Some(OccupancyApi::A2s), Some(_), _) => bail!(
            "SUPERVISOR_OCCUPANCY_API is 'a2s', which is addressed by SUPERVISOR_A2S_PORT, \
             but SUPERVISOR_OCCUPANCY_API_URL is set"
        ),
        (Some(OccupancyApi::Satisfactory | OccupancyApi::Tshock) | None, _, Some(_)) => {
            bail!("SUPERVISOR_A2S_PORT is set but SUPERVISOR_OCCUPANCY_API is not 'a2s'")
        }
        (Some(OccupancyApi::Satisfactory) | None, Some(url), None) => {
            Ok(Some(OccupancyEndpoint::Satisfactory(url)))
        }
        (Some(OccupancyApi::Tshock), Some(url), None) => Ok(Some(OccupancyEndpoint::Tshock(url))),
        (Some(api @ (OccupancyApi::Satisfactory | OccupancyApi::Tshock)), None, None) => {
            bail!("SUPERVISOR_OCCUPANCY_API is {api:?} but SUPERVISOR_OCCUPANCY_API_URL is not set")
        }
        (None, None, None) => Ok(None),
    }
}

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

fn parse_child_env(lookup: EnvLookup) -> Option<ChildEnvOverlay> {
    let file = optional(lookup, "SUPERVISOR_CHILD_ENV_FILE")?;
    let allowlist = optional(lookup, "SUPERVISOR_CHILD_ENV_ALLOWLIST")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    Some(ChildEnvOverlay { file, allowlist })
}

fn parse_thunderstore(lookup: EnvLookup) -> Result<Option<ThunderstoreMods>> {
    let file = optional(lookup, "SUPERVISOR_MODS_FILE");
    let community = optional(lookup, "SUPERVISOR_THUNDERSTORE_COMMUNITY");
    let (file, community) = match (file, community) {
        (Some(file), Some(community)) => (file, community),
        (None, None) => return Ok(None),
        (Some(file), None) => {
            warn!(
                file = %file,
                "SUPERVISOR_MODS_FILE is set but SUPERVISOR_THUNDERSTORE_COMMUNITY is not; \
                 mod installs are off for this game"
            );
            return Ok(None);
        }
        (None, Some(community)) => {
            warn!(
                community = %community,
                "SUPERVISOR_THUNDERSTORE_COMMUNITY is set but SUPERVISOR_MODS_FILE is not; \
                 mod installs are off for this game"
            );
            return Ok(None);
        }
    };
    let layout = match optional(lookup, "SUPERVISOR_THUNDERSTORE_LAYOUT") {
        Some(raw) => PackageLayout::parse(&raw).ok_or_else(|| {
            anyhow!("SUPERVISOR_THUNDERSTORE_LAYOUT must be 'bepinex', got {raw:?}")
        })?,
        None => PackageLayout::BepInEx,
    };
    let provided = match optional(lookup, "SUPERVISOR_THUNDERSTORE_PROVIDED") {
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                entry.parse::<PackageId>().map_err(|err| {
                    anyhow!("SUPERVISOR_THUNDERSTORE_PROVIDED entry {entry:?} is invalid: {err}")
                })
            })
            .collect::<Result<Vec<_>>>()?,
        None => Vec::new(),
    };
    let work_dir = match optional(lookup, "SUPERVISOR_THUNDERSTORE_DIR") {
        Some(dir) if dir != DEFAULT_THUNDERSTORE_DIR => {
            warn!(
                dir = %dir,
                default = DEFAULT_THUNDERSTORE_DIR,
                "SUPERVISOR_THUNDERSTORE_DIR overrides the shared default; \
                 the bot will not find this instance's resolved mod set"
            );
            dir
        }
        Some(dir) => dir,
        None => DEFAULT_THUNDERSTORE_DIR.to_owned(),
    };
    let config_dir = optional(lookup, "SUPERVISOR_THUNDERSTORE_CONFIG_DIR");
    let config_keep = match optional(lookup, "SUPERVISOR_THUNDERSTORE_CONFIG_KEEP") {
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_owned)
            .collect(),
        None => Vec::new(),
    };
    if config_dir.is_none() && !config_keep.is_empty() {
        warn!(
            "SUPERVISOR_THUNDERSTORE_CONFIG_KEEP is set but \
             SUPERVISOR_THUNDERSTORE_CONFIG_DIR is not; \
             nothing sweeps mod settings for this game, so it protects nothing"
        );
    }
    Ok(Some(ThunderstoreMods {
        file,
        community,
        layout,
        provided,
        work_dir,
        config_dir,
        config_keep,
    }))
}

fn warn_if_child_env_half_configured(lookup: EnvLookup, child_env: Option<&ChildEnvOverlay>) {
    match child_env {
        Some(overlay) if overlay.allowlist.is_empty() => warn!(
            file = %overlay.file,
            "SUPERVISOR_CHILD_ENV_FILE is set but SUPERVISOR_CHILD_ENV_ALLOWLIST is empty; \
             the per-instance env overlay will inject nothing"
        ),
        Some(_) => {}
        None => {
            if optional(lookup, "SUPERVISOR_CHILD_ENV_ALLOWLIST").is_some() {
                warn!(
                    "SUPERVISOR_CHILD_ENV_ALLOWLIST is set but SUPERVISOR_CHILD_ENV_FILE is not; \
                     the per-instance env overlay is inert"
                );
            }
        }
    }
}

fn warn_if_rcon_settings_without_port(lookup: EnvLookup, rcon_port: Option<u16>) {
    if rcon_port.is_some() {
        return;
    }
    let rcon_dependent_keys = [
        "SUPERVISOR_RCON_DIALECT",
        "SUPERVISOR_RCON_PASSWORD_ENV",
        "SUPERVISOR_RCON_PASSWORD_MAX_LEN",
        "SUPERVISOR_PALWORLD_INI",
        "SUPERVISOR_AUTO_UPDATE",
    ];
    let mut configured = Vec::new();
    for key in rcon_dependent_keys {
        if optional(lookup, key).is_some() {
            configured.push(key);
        }
    }
    if configured.is_empty() {
        return;
    }
    warn!(
        settings = %configured.join(", "),
        "rcon-dependent settings set but SUPERVISOR_RCON_PORT is not; rcon is \
         disabled so they won't take effect"
    );
}

fn optional(lookup: EnvLookup, key: &str) -> Option<String> {
    lookup(key).and_then(|raw| raw.into_string().ok())
}

fn optional_flag(lookup: EnvLookup, key: &str) -> bool {
    optional_flag_or(lookup, key, false)
}

fn optional_flag_or(lookup: EnvLookup, key: &str, default: bool) -> bool {
    match optional(lookup, key) {
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
}

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
