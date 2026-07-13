use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tracing::warn;

/// Closure that resolves an environment variable to its raw value, mirroring
/// `std::env::var_os`. Injected so config parsing is testable without the
/// `unsafe` `set_var`/`remove_var` of Rust 2024.
pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

const DEFAULT_NAMESPACE: &str = "game-servers";
const DEFAULT_DOMAIN: &str = "gameservers.grizzly-endeavors.com";
/// Where the per-game catalog is baked into the container image (see Dockerfile).
const DEFAULT_CATALOG_DIR: &str = "/usr/local/share/grizzly-gameservers/games";
/// Port the in-pod supervisor serves its control API on. Sourced from the shared
/// `grizzly_control_api::CONTROL_PORT` (the single Rust source of truth) so the bot
/// and supervisor can't drift from each other. Still must match the catalog
/// (`games/<game>/gameserver.yaml`) and the hardcoded port in
/// `cluster/guardrails/bot-to-supervisor-egress.yaml` by hand — those YAML files
/// can't import a Rust const, so that half stays tracked under issue #42; Cilium
/// silently drops on a mismatch instead of erroring.
const DEFAULT_CONTROL_PORT: u16 = grizzly_control_api::CONTROL_PORT;
/// Port the in-game agent endpoint listens on for supervisor-posted `@Gary`
/// triggers. Pod-internal, reached over the bot's `ClusterIP` Service. Must
/// match the hardcoded port in
/// `cluster/guardrails/game-to-bot-agent-egress.yaml` (see issue #42).
const DEFAULT_AGENT_PORT: u16 = 9360;
/// Ollama Cloud's `OpenAI`-compatible chat-completions base. The agent ("Gary")
/// posts to `{base}/chat/completions`. Overridable for self-hosted Ollama.
const DEFAULT_OLLAMA_BASE_URL: &str = "https://ollama.com/v1";
/// Default model tag the agent drives — GLM 5.2 via Ollama Cloud unless overridden.
const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2";
/// Foundation Postgres on the R730xd (ADR-003). LAN-only, plain TCP; overridable
/// for local dev against a different host. Must match the hardcoded
/// host/port in `cluster/guardrails/bot-to-postgres-egress.yaml` (see issue
/// #42).
const DEFAULT_DB_HOST: &str = "10.0.0.200";
const DEFAULT_DB_PORT: u16 = 5432;
/// The bot's dedicated role and (role-owned) database on foundation Postgres,
/// provisioned by `setup-grizzly-gameservers-stores.yml`.
const DEFAULT_DB_NAME: &str = "grizzly_gameservers";
const DEFAULT_DB_USER: &str = "grizzly_gameservers";
/// Foundation Valkey (kv-cache) on the R730xd — the shared Redis-wire store the
/// platform designates for light queues/caches. LAN-only, plain TCP. Backs Gary's
/// deferred-task queue. Must match the hardcoded host/port in
/// `cluster/guardrails/bot-to-kv-cache-egress.yaml` (see issue #42).
const DEFAULT_REDIS_HOST: &str = "10.0.0.200";
const DEFAULT_REDIS_PORT: u16 = 6379;
/// Logical DB index for the deferred-task queue. The shared instance has one
/// password (no ACL users), so isolation is by key prefix; the DB index is a
/// courtesy that keeps a `SCAN` cheap. `0` is the default shared DB and `1` is
/// Authentik's — this app takes `2`.
const DEFAULT_REDIS_DB: u8 = 2;
/// Self-hosted versitygw `s3-bulk` on the R730xd — the endpoint the platform S3
/// doc designates for backups/archives. Path-style, plain HTTP over the LAN.
/// Must match the hardcoded host/port in
/// `cluster/guardrails/bot-to-s3-egress.yaml` (see issue #42).
const DEFAULT_S3_ENDPOINT: &str = "http://10.0.0.200:7072";
const DEFAULT_S3_BUCKET: &str = "grizzly-gameservers";
const DEFAULT_S3_REGION: &str = "us-east-1";
/// How often the scheduled cycle backs up every running server, in hours.
const DEFAULT_BACKUP_INTERVAL_HOURS: u64 = 24;
/// How many backups to retain per server; older ones are pruned each cycle.
const DEFAULT_BACKUP_RETENTION: usize = 7;

/// Runtime configuration for the bot, sourced from the process environment.
#[derive(Clone, Debug)]
pub struct BotConfig {
    pub(crate) token: String,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) catalog_dir: PathBuf,
    /// Port the in-pod supervisor's control API listens on.
    pub(crate) control_port: u16,
    /// Cross-guild operator seed: user ids that are admins in **every** guild and
    /// carry the all-guilds visibility scope. Per-guild admins live in Postgres
    /// (`GuildConfig`); this env allowlist is only the bootstrap operator(s).
    /// Sourced from `GAMESERVERS_ADMIN_USER_IDS`.
    pub(crate) operator_ids: Vec<u64>,
    /// Ollama Cloud API key for the agent ("Gary"). Absent disables the agent —
    /// mentions are answered with a "not configured" reply, slash commands still
    /// work. Sourced from the `ollama-api` Secret's `api_key` in-cluster.
    pub(crate) ollama_api_key: Option<String>,
    /// Base URL for the agent's `OpenAI`-compatible chat-completions endpoint.
    pub(crate) ollama_base_url: String,
    /// Model tag the agent drives.
    pub(crate) ollama_model: String,
    /// Foundation-Postgres connection for the bot's durable state. `None` when no
    /// `DB_PASSWORD` is set — the bot then runs without persistence (no-mention
    /// home channels are disabled), the same graceful-degrade shape as `ollama`.
    pub(crate) db: Option<DbConfig>,
    /// Foundation-Valkey connection for Gary's deferred-task queue. `None` when no
    /// `REDIS_PASSWORD` is set — the `run_when` tool then reports it can't schedule,
    /// the same graceful-degrade shape as `db`/`ollama`.
    pub(crate) valkey: Option<ValkeyConfig>,
    /// S3 (versitygw) connection for backups/archives. `None` when the access/
    /// secret keys aren't set — backups/archive/restore then report "not
    /// configured", the same graceful-degrade shape as `db`/`ollama`.
    pub(crate) s3: Option<S3Config>,
    /// How often the scheduled backup cycle runs.
    pub(crate) backup_interval: Duration,
    /// How many backups to keep per server before the cycle prunes older ones.
    pub(crate) backup_retention: usize,
    /// Port the in-game agent endpoint binds for supervisor-posted chat triggers.
    pub(crate) agent_port: u16,
    /// Shared bearer token the in-game agent endpoint requires. `None` runs it
    /// open (NetworkPolicy-only); synced from `OpenBao` via ESO in-cluster.
    pub(crate) ingame_token: Option<String>,
}

/// Connection settings for the backups S3 bucket. Only the access/secret keys are
/// secrets (synced from `OpenBao` via ESO); the rest carry infra defaults.
#[derive(Clone, Debug)]
pub(crate) struct S3Config {
    pub(crate) endpoint: String,
    pub(crate) bucket: String,
    pub(crate) region: String,
    pub(crate) access_key: String,
    pub(crate) secret_key: String,
}

/// Connection settings for the bot's foundation-Valkey (kv-cache) queue. Only the
/// password is a secret (synced from `OpenBao`); the rest carry infra defaults.
#[derive(Clone, Debug)]
pub(crate) struct ValkeyConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) db: u8,
    pub(crate) password: String,
}

impl ValkeyConfig {
    /// The `redis://` connection URL, with the password after the empty username
    /// (`redis://:<password>@host:port/db`) — the RESP auth form the shared
    /// `requirepass` instance expects.
    pub(crate) fn url(&self) -> String {
        format!(
            "redis://:{}@{}:{}/{}",
            self.password, self.host, self.port, self.db
        )
    }
}

/// Connection settings for the bot's foundation-Postgres database. Only the
/// password is a secret (synced from `OpenBao`); the rest carry infra defaults.
#[derive(Clone, Debug)]
pub(crate) struct DbConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) user: String,
    pub(crate) password: String,
}

impl BotConfig {
    /// Build configuration from the real process environment.
    ///
    /// # Errors
    ///
    /// Returns an error if `DISCORD_BOT_TOKEN` is unset or non-UTF-8.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with(&|key| std::env::var_os(key))
    }

    pub(crate) fn from_env_with(lookup: EnvLookup) -> Result<Self> {
        let token = required(lookup, "DISCORD_BOT_TOKEN")?;

        let namespace = optional(lookup, "GAMESERVERS_NAMESPACE")
            .unwrap_or_else(|| DEFAULT_NAMESPACE.to_owned());
        let domain =
            optional(lookup, "GAMESERVERS_DOMAIN").unwrap_or_else(|| DEFAULT_DOMAIN.to_owned());
        let catalog_dir = optional(lookup, "GAMESERVERS_CATALOG_DIR")
            .map_or_else(|| PathBuf::from(DEFAULT_CATALOG_DIR), PathBuf::from);
        let control_port =
            optional_port(lookup, "GAMESERVERS_CONTROL_PORT")?.unwrap_or(DEFAULT_CONTROL_PORT);
        let operator_ids =
            parse_user_ids(optional(lookup, "GAMESERVERS_ADMIN_USER_IDS").as_deref())?;

        let ollama_api_key = optional(lookup, "OLLAMA_API_KEY").filter(|key| !key.is_empty());
        let ollama_base_url = optional(lookup, "OLLAMA_BASE_URL")
            .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_owned());
        let ollama_model =
            optional(lookup, "OLLAMA_MODEL").unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned());

        let db = db_config_from_env(lookup)?;
        let valkey = valkey_config_from_env(lookup)?;
        let s3 = s3_config_from_env(lookup);
        let backup_interval = Duration::from_secs(
            optional_positive_u64(lookup, "GAMESERVERS_BACKUP_INTERVAL_HOURS")?
                .unwrap_or(DEFAULT_BACKUP_INTERVAL_HOURS)
                .saturating_mul(3600),
        );
        let backup_retention = optional_positive_usize(lookup, "GAMESERVERS_BACKUP_RETENTION")?
            .unwrap_or(DEFAULT_BACKUP_RETENTION);
        let agent_port =
            optional_port(lookup, "GAMESERVERS_AGENT_PORT")?.unwrap_or(DEFAULT_AGENT_PORT);
        let ingame_token =
            optional(lookup, "GAMESERVERS_INGAME_TOKEN").filter(|value| !value.is_empty());

        Ok(Self {
            token,
            namespace,
            domain,
            catalog_dir,
            control_port,
            operator_ids,
            ollama_api_key,
            ollama_base_url,
            ollama_model,
            db,
            valkey,
            s3,
            backup_interval,
            backup_retention,
            agent_port,
            ingame_token,
        })
    }
}

/// Build the S3 connection settings from the environment, or `None` when either
/// key is unset/empty — the keys are the parts sourced from `OpenBao`, so their
/// absence is the signal that backups aren't wired and the feature should degrade
/// rather than fail.
fn s3_config_from_env(lookup: EnvLookup) -> Option<S3Config> {
    let access_key =
        optional(lookup, "GAMESERVERS_S3_ACCESS_KEY").filter(|value| !value.is_empty());
    let secret_key =
        optional(lookup, "GAMESERVERS_S3_SECRET_KEY").filter(|value| !value.is_empty());
    let (access_key, secret_key) = (access_key?, secret_key?);
    Some(S3Config {
        endpoint: optional(lookup, "GAMESERVERS_S3_ENDPOINT")
            .unwrap_or_else(|| DEFAULT_S3_ENDPOINT.to_owned()),
        bucket: optional(lookup, "GAMESERVERS_S3_BUCKET")
            .unwrap_or_else(|| DEFAULT_S3_BUCKET.to_owned()),
        region: optional(lookup, "GAMESERVERS_S3_REGION")
            .unwrap_or_else(|| DEFAULT_S3_REGION.to_owned()),
        access_key,
        secret_key,
    })
}

/// Build the Postgres connection settings from the environment, or `None` when
/// `DB_PASSWORD` is unset/empty — the password is the one part sourced from
/// `OpenBao`, so its absence is the signal that persistence isn't wired and the
/// bot should degrade rather than fail.
///
/// # Errors
///
/// Returns an error if `DB_PORT` is set but not a valid port number.
fn db_config_from_env(lookup: EnvLookup) -> Result<Option<DbConfig>> {
    let Some(password) = optional(lookup, "DB_PASSWORD").filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    Ok(Some(DbConfig {
        host: optional(lookup, "DB_HOST").unwrap_or_else(|| DEFAULT_DB_HOST.to_owned()),
        port: optional_port(lookup, "DB_PORT")?.unwrap_or(DEFAULT_DB_PORT),
        database: optional(lookup, "DB_NAME").unwrap_or_else(|| DEFAULT_DB_NAME.to_owned()),
        user: optional(lookup, "DB_USER").unwrap_or_else(|| DEFAULT_DB_USER.to_owned()),
        password,
    }))
}

/// Build the Valkey connection settings from the environment, or `None` when
/// `REDIS_PASSWORD` is unset/empty — the password is the one part sourced from
/// `OpenBao`, so its absence is the signal that the queue backend isn't wired and
/// the deferred-task feature should degrade (Gary reports he can't schedule)
/// rather than fail.
///
/// # Errors
///
/// Returns an error if `REDIS_PORT` is set but not a valid port, or `REDIS_DB` is
/// set but not a valid DB index (0-15).
fn valkey_config_from_env(lookup: EnvLookup) -> Result<Option<ValkeyConfig>> {
    let Some(password) = optional(lookup, "REDIS_PASSWORD").filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(ValkeyConfig {
        host: optional(lookup, "REDIS_HOST").unwrap_or_else(|| DEFAULT_REDIS_HOST.to_owned()),
        port: optional_port(lookup, "REDIS_PORT")?.unwrap_or(DEFAULT_REDIS_PORT),
        db: optional_redis_db(lookup, "REDIS_DB")?.unwrap_or(DEFAULT_REDIS_DB),
        password,
    }))
}

/// Parse an optional env var as a Redis logical DB index (0-15). Rejects
/// non-numeric values and out-of-range indices — the shared instance runs the
/// default 16 databases (0-15).
fn optional_redis_db(lookup: EnvLookup, key: &str) -> Result<Option<u8>> {
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    let value = raw
        .parse::<u8>()
        .ok()
        .filter(|&value| value <= 15)
        .with_context(|| format!("{key} must be a Redis DB index (0-15), got {raw:?}"))?;
    Ok(Some(value))
}

/// Parse an optional env var as a positive `u64` (>= 1). Rejects both non-numeric
/// values and a degenerate `0`: a zero backup interval flows into
/// `tokio::time::interval`, which panics on a zero period.
fn optional_positive_u64(lookup: EnvLookup, key: &str) -> Result<Option<u64>> {
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    let value = raw
        .parse::<u64>()
        .ok()
        .filter(|&value| value != 0)
        .with_context(|| format!("{key} must be a positive integer (>= 1), got {raw:?}"))?;
    Ok(Some(value))
}

/// Parse an optional env var as a positive `usize` (>= 1). Rejects both
/// non-numeric values and a degenerate `0`: a zero retention prunes every key
/// each cycle, so backups appear to run but nothing is ever kept.
fn optional_positive_usize(lookup: EnvLookup, key: &str) -> Result<Option<usize>> {
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    let value = raw
        .parse::<usize>()
        .ok()
        .filter(|&value| value != 0)
        .with_context(|| format!("{key} must be a positive integer (>= 1), got {raw:?}"))?;
    Ok(Some(value))
}

/// Parse an optional env var as a usable port (1-65535). Rejects `0`, which
/// parses as a valid `u16` but is never a usable port to bind or connect to.
fn optional_port(lookup: EnvLookup, key: &str) -> Result<Option<u16>> {
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    let port = raw
        .parse::<u16>()
        .ok()
        .filter(|&port| port != 0)
        .with_context(|| format!("{key} must be a port number (1-65535), got {raw:?}"))?;
    Ok(Some(port))
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
    match lookup(key)?.into_string() {
        Ok(value) => Some(value),
        // Mirror `required`'s fail-loud stance: a set-but-mangled value shouldn't
        // vanish into the default with no trace, or a misconfiguration reads as
        // "unset" and is impossible to spot.
        Err(bad) => {
            warn!(
                key,
                value = %bad.display(),
                "ignoring non-UTF-8 value for optional env var; falling back to default"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;
