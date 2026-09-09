use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tracing::warn;

use crate::autoupdate::AutoUpdatePolicy;
use crate::chat_watcher::ChatFormat;
use crate::occupancy::OccupancyApi;
use crate::rcon::RconDialect;
use crate::thunderstore::archive::PackageLayout;
use crate::thunderstore::types::PackageId;

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
/// Default control-API port. Sourced from the shared
/// `grizzly_control_api::CONTROL_PORT` (the single Rust source of truth, where the
/// port-choice rationale lives) so the bot and supervisor can't drift from each
/// other. The `cluster/guardrails/bot-to-supervisor-egress.yaml` carve-out still
/// hardcodes the same value and must be matched by hand — a YAML file can't import
/// a Rust const, so that half stays tracked under issue #42; a mismatch there is a
/// silent Cilium drop, not an error.
const DEFAULT_CONTROL_PORT: u16 = grizzly_control_api::CONTROL_PORT;
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
/// Shared with the bot and the game manifests' `terminationGracePeriodSeconds`,
/// both of which have to clear this window.
const DEFAULT_GRACEFUL_TIMEOUT_SECS: u64 = grizzly_control_api::DEFAULT_GRACEFUL_TIMEOUT_SECS;
/// Sliding window over which repeated crashes count toward escalation.
const DEFAULT_CRASH_WINDOW_SECS: u64 = 300;
/// Crashes within the window before the supervisor stops the heartbeat and lets
/// Agones recreate the pod.
const DEFAULT_CRASH_THRESHOLD: u32 = 5;
/// Child env var the minted RCON password is injected under. Matches itzg's
/// `RCON_PASSWORD`; a Source game overrides it to whatever its entrypoint reads.
const DEFAULT_RCON_PASSWORD_ENV: &str = "RCON_PASSWORD";
/// How often the update-on-empty loop polls occupancy. Well under the empty-grace
/// window so a short idle streak is still sampled several times before it acts.
const DEFAULT_AUTO_UPDATE_POLL_SECS: u64 = 120;
/// How long a server must stay empty before an idle update-relaunch fires.
const DEFAULT_AUTO_UPDATE_EMPTY_GRACE_SECS: u64 = 300;
/// Minimum build age before an idle server is worth bouncing to re-pull (24h).
const DEFAULT_AUTO_UPDATE_INTERVAL_SECS: u64 = 86_400;
/// Hard cap on build age: past this, relaunch even while occupied (3 days).
const DEFAULT_AUTO_UPDATE_MAX_UPTIME_SECS: u64 = 259_200;
/// Where the mod reconciler keeps its cache, staged tree, and `resolved` file,
/// relative to the data root. Under the same reserved `.grizzly/` directory as
/// the env overlay, so everything the platform owns on an instance's volume sits
/// in one place a friend can recognise as not-theirs.
const DEFAULT_THUNDERSTORE_DIR: &str = grizzly_control_api::mods::DEFAULT_WORK_DIR;

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
    /// Shared secret the bot must present on the control API, or `None` to accept
    /// any caller the network leash already allowed through. Absent by default so
    /// an image or deployment without the secret keeps working rather than locking
    /// the bot out of every server at once.
    pub control_token: Option<String>,
    /// Sliding window for crash-rate escalation.
    pub crash_window: Duration,
    /// Crash count within `crash_window` that triggers escalation.
    pub crash_threshold: u32,
    /// Base URL of the game's own occupancy API, or `None` for games that report
    /// it over RCON. Its presence selects the alternate source, which is the only
    /// way Satisfactory and Terraria get a player count at all — neither has a
    /// console, and without a count neither can join update-on-empty.
    pub occupancy_api_url: Option<String>,
    /// Which API the URL above speaks. Ignored when there is no URL.
    pub occupancy_api: OccupancyApi,
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
    /// Per-instance child-environment overlay, or `None` when the game's catalog
    /// entry doesn't opt in (no `SUPERVISOR_CHILD_ENV_FILE`).
    pub child_env: Option<ChildEnvOverlay>,
    /// Pre-launch Thunderstore mod reconciliation, or `None` when this game
    /// isn't one of the r2modman-shaped games (no `SUPERVISOR_MODS_FILE`, or no
    /// `SUPERVISOR_THUNDERSTORE_COMMUNITY` in the image).
    pub thunderstore: Option<ThunderstoreMods>,
    /// Whether the update-on-empty auto-updater runs. On by default; it only has
    /// any effect when the game has an occupancy source (RCON, or the Satisfactory
    /// API via `occupancy_api_url`), since the loop needs a player count to prove a
    /// server is empty before bouncing it — a game with neither is reactive-only
    /// regardless. Set `SUPERVISOR_AUTO_UPDATE` to a falsy value to pin a server to
    /// its current build.
    pub auto_update_enabled: bool,
    /// How often the auto-updater polls occupancy.
    pub auto_update_poll_interval: Duration,
    /// Thresholds the auto-updater applies to decide when to relaunch for an update.
    pub auto_update_policy: AutoUpdatePolicy,
}

/// The game's opt-in to the per-instance child-environment overlay: where the
/// file lives and which keys it may set. Built as a unit so the injection path
/// can't be handed a file with no policy attached — an absent allowlist is still
/// represented here (as empty), and injects nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildEnvOverlay {
    /// Overlay file path, relative to [`SupervisorConfig::data_dir`].
    pub file: String,
    /// Keys the overlay may set, matched case-sensitively and exactly. Empty
    /// means nothing is injected — the mechanism fails closed rather than open.
    pub allowlist: Vec<String>,
}

/// The game's opt-in to pre-launch Thunderstore mod reconciliation. Built as a
/// unit for the same reason as [`ChildEnvOverlay`]: the reconciler can't be
/// handed a mod list without the community it is written against or the rule for
/// where a package's files go.
///
/// The split across two config sources is deliberate and mirrors the env
/// overlay's: the *file* is per-game policy and lives in the catalog manifest,
/// where changing it is a reviewable edit that lands on the next `/create`;
/// everything else is a fact about the image and lives in its Dockerfile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThunderstoreMods {
    /// Mod-list path, relative to [`SupervisorConfig::data_dir`].
    pub file: String,
    /// The Thunderstore community this game's mods come from. Packages never
    /// cross between communities, so this names which catalog the mod list is
    /// written against; it is logged at startup so a mismatch between the image
    /// and the bot's catalog annotation is visible in one place.
    pub community: String,
    /// Which per-game rule maps a package's files onto the loader's directories.
    pub layout: PackageLayout,
    /// Packages the image already supplies (the loader pack), skipped rather
    /// than installed. Matched by `Namespace-Name`, ignoring version: the image
    /// owns whichever version it shipped.
    pub provided: Vec<PackageId>,
    /// Working directory for the cache, staged tree, and `resolved` file,
    /// relative to [`SupervisorConfig::data_dir`].
    pub work_dir: String,
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
        let control_token = optional(lookup, "SUPERVISOR_CONTROL_TOKEN");
        let occupancy_api_url = optional(lookup, "SUPERVISOR_OCCUPANCY_API_URL");
        let occupancy_api = parse_occupancy_api(lookup)?;
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
            occupancy_api_url,
            occupancy_api,
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

/// Assemble the chat-watch config, or `None` when the game doesn't opt in
/// (`SUPERVISOR_CHAT_FORMAT` unset). Opting in but omitting the endpoint
/// (`SUPERVISOR_AGENT_URL`) or the server identity (`SUPERVISOR_GAMESERVER_NAME`)
/// is a hard error rather than a silent disable — a half-configured watcher would
/// swallow every `@Gary` with no signal.
/// Which occupancy API this image's `SUPERVISOR_OCCUPANCY_API_URL` speaks.
///
/// An unrecognised value is a hard error rather than a silent fallback: the two
/// dialects are different protocols, so a typo that quietly picked the wrong one
/// would leave the game reporting unknown occupancy forever with nothing pointing
/// at the cause.
fn parse_occupancy_api(lookup: EnvLookup) -> Result<OccupancyApi> {
    match optional(lookup, "SUPERVISOR_OCCUPANCY_API") {
        None => Ok(OccupancyApi::default()),
        Some(raw) => OccupancyApi::parse(&raw).ok_or_else(|| {
            anyhow!("SUPERVISOR_OCCUPANCY_API must be 'satisfactory' or 'tshock', got {raw:?}")
        }),
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

/// Assemble the child-env overlay config, or `None` when the game doesn't opt in
/// (`SUPERVISOR_CHILD_ENV_FILE` unset). The allowlist is comma-separated; entries
/// are trimmed and blank ones dropped, so a trailing comma or a line-wrapped YAML
/// value doesn't produce a key nothing can ever match.
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

/// Assemble the Thunderstore mod-reconciler config, or `None` when the game
/// doesn't opt in. Both halves are required together, and they come from
/// different places on purpose (see [`ThunderstoreMods`]): the mod list is
/// declared in the catalog manifest, the community in the image. Either alone
/// warns and disables the reconciler — the same posture as the env overlay,
/// and for the same reason: refusing to start would take down a live server on
/// its next restart over a config mistake that only costs it mod support.
///
/// A *bad* value, by contrast, is a hard error. An unknown layout or an
/// unparseable provided-package name would each silently install mods into the
/// wrong place or install a second loader pack over the image's — failures that
/// look like the game being broken rather than like a typo.
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
            // The bot addresses `resolved` and the cached profile code by path,
            // and it takes that path from the shared default because a
            // Dockerfile ENV never reaches it. An override still works for
            // everything in-pod; it just makes those two files unreadable from
            // outside, so it is said out loud rather than discovered later.
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
    Ok(Some(ThunderstoreMods {
        file,
        community,
        layout,
        provided,
        work_dir,
    }))
}

/// Warn on either half-configured state of the overlay pair: a file with no
/// allowlist (injects nothing, since the mechanism fails closed) or an allowlist
/// with no file (nothing to read). Both look like "working, no overrides" from
/// the outside, so a catalog typo would otherwise only surface as a mod install
/// that quietly never takes effect. A warning rather than a hard error, for the
/// same reason as [`warn_if_rcon_settings_without_port`].
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

/// Warn when one or more RCON-dependent settings (`rcon_dialect`,
/// `rcon_password_env`, `rcon_password_max_len`, `palworld_ini_path`,
/// `auto_update_enabled`) are set but `SUPERVISOR_RCON_PORT` is absent. All of
/// them are parsed unconditionally but silently do nothing without a port — no
/// RCON seeding, no `/command` route — so this makes that misconfiguration
/// visible at startup instead of only when someone tries RCON and finds it
/// missing.
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
    // parse_chat_watch treats its half-configured case as a hard error. That's
    // the stricter alternative here too, but this is a live system: refusing to
    // start would take down a currently-running (if degraded) server on its next
    // restart, so this stays a warning rather than matching that behavior.
    warn!(
        settings = %configured.join(", "),
        "rcon-dependent settings set but SUPERVISOR_RCON_PORT is not; rcon is \
         disabled so they won't take effect"
    );
}

fn optional(lookup: EnvLookup, key: &str) -> Option<String> {
    lookup(key).and_then(|raw| raw.into_string().ok())
}

/// Interpret an optional flag variable as a boolean, accepting the common truthy
/// spellings case-insensitively. Absent or unrecognized values are `false`.
fn optional_flag(lookup: EnvLookup, key: &str) -> bool {
    optional_flag_or(lookup, key, false)
}

/// Interpret an optional flag variable as a boolean, falling back to `default`
/// when it's absent (so a flag that ships enabled can still be turned off). A set
/// value is read with the same truthy spellings as [`optional_flag`]; anything
/// else reads as `false`.
fn optional_flag_or(lookup: EnvLookup, key: &str, default: bool) -> bool {
    match optional(lookup, key) {
        Some(raw) => matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        None => default,
    }
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
