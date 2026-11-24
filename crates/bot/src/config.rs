use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use tracing::warn;

use crate::agent::Environment;
use crate::events::{parse_bytes, parse_cpu_millicores};

pub(crate) type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<OsString>;

const DEFAULT_NAMESPACE: &str = "game-servers";
const DEFAULT_DOMAIN: &str = "gameservers.grizzly-endeavors.com";
const DEFAULT_CATALOG_DIR: &str = "/usr/local/share/grizzly-gameservers/games";
const DEFAULT_CONTROL_PORT: u16 = grizzly_control_api::CONTROL_PORT;
const DEFAULT_AGENT_PORT: u16 = 9360;
const DEFAULT_OLLAMA_BASE_URL: &str = "https://ollama.com/v1";
const DEFAULT_OLLAMA_MODEL: &str = "glm-5.2";
const DEFAULT_DB_HOST: &str = "10.0.0.200";
const DEFAULT_DB_PORT: u16 = 5432;
const DEFAULT_DB_NAME: &str = "grizzly_gameservers";
const DEFAULT_DB_USER: &str = "grizzly_gameservers";
const DEFAULT_REDIS_HOST: &str = "10.0.0.200";
const DEFAULT_REDIS_PORT: u16 = 6379;
const DEFAULT_REDIS_DB: u8 = 2;
const DEFAULT_S3_ENDPOINT: &str = "http://10.0.0.200:7072";
const DEFAULT_S3_BUCKET: &str = "grizzly-gameservers";
const DEFAULT_S3_REGION: &str = "us-east-1";
/// The platform's Stalwart submission endpoint. Deliberately the **public** host
/// even though the bot runs in-cluster: the listeners trust PROXY-protocol from
/// `10.0.0.0/8`, which covers the pod network, so a direct in-cluster connection
/// is parsed as a PROXY header and reset (`errno 104`). The public path rides the
/// VPS `HAProxy`, which prepends the header correctly. See grizzly-platform
/// `docs/integration/mail.md`.
const DEFAULT_SMTP_HOST: &str = "mail.grizzly-endeavors.com";
const DEFAULT_SMTP_PORT: u16 = 587;
/// The submission account provisioned for this app. Must stay on
/// `grizzly-endeavors.com`: DMARC alignment is anchored there, and a `From:` on
/// any other domain is filtered rather than delivered.
const DEFAULT_SMTP_USER: &str = "gary@grizzly-endeavors.com";
const DEFAULT_BACKUP_INTERVAL_HOURS: u64 = 24;
const DEFAULT_BACKUP_RETENTION: usize = 7;
const DEFAULT_ARCHIVE_RETENTION_DAYS: u64 = 90;
const DEFAULT_RECONCILE_INTERVAL_MINUTES: u64 = 5;
const DEFAULT_SAMPLE_INTERVAL_MINUTES: u64 = 5;
const IN_CLUSTER_MARKER: &str = "KUBERNETES_SERVICE_HOST";
const DEFAULT_LANGFUSE_HOST: &str = "http://langfuse-web.langfuse.svc.cluster.local:3000";
const DEFAULT_ADMIN_GROUP: &str = "grizzly-admins";
const DEFAULT_ADMIN_ORIGIN: &str = "https://gameservers-admin.grizzly-endeavors.com";
const DEFAULT_RELEASE: &str = "dev";
const DEFAULT_STRANDED_DWELL_MINUTES: u64 = 15;
const DEFAULT_GUILD_MAX_SERVERS: u32 = 5;
const DEFAULT_GUILD_MAX_CPU_MILLICORES: i64 = 24_000;
const DEFAULT_GUILD_MAX_MEM_BYTES: i64 = 48 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WatchdogMode {
    On,
    Shadow,
    Off,
}

impl WatchdogMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Shadow => "shadow",
            Self::Off => "off",
        }
    }
}

pub(crate) fn parse_watchdog_mode(raw: &str) -> Option<WatchdogMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "on" => Some(WatchdogMode::On),
        "shadow" => Some(WatchdogMode::Shadow),
        "off" => Some(WatchdogMode::Off),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct BotConfig {
    pub(crate) token: String,
    pub(crate) namespace: String,
    pub(crate) domain: String,
    pub(crate) catalog_dir: PathBuf,
    pub(crate) control_port: u16,
    pub(crate) operator_ids: Vec<u64>,
    pub(crate) guild_allowlist: Vec<u64>,
    pub(crate) beta_invite_url: String,
    pub(crate) community_url: String,
    pub(crate) throttle: crate::throttle::ThrottlePolicy,
    pub(crate) ollama_api_key: Option<String>,
    pub(crate) ollama_base_url: String,
    pub(crate) ollama_model: String,
    pub(crate) db: Option<DbConfig>,
    pub(crate) valkey: Option<ValkeyConfig>,
    pub(crate) supervisor_token: Option<String>,
    pub(crate) s3: Option<S3Config>,
    pub(crate) mail: Option<MailConfig>,
    pub(crate) backup_interval: Duration,
    pub(crate) backup_retention: usize,
    pub(crate) archive_retention_days: i32,
    pub(crate) reconcile_interval: Duration,
    pub(crate) stranded_dwell: Duration,
    pub(crate) sample_interval: Duration,
    pub(crate) agent_port: u16,
    pub(crate) ingame_token: Option<String>,
    pub(crate) operator_token: Option<String>,
    pub(crate) signup_token: Option<String>,
    pub(crate) environment: Environment,
    pub(crate) langfuse: Option<LangfuseConfig>,
    pub(crate) release: String,
    pub(crate) watchdog: WatchdogMode,
    pub(crate) guild_limits: GuildLimitDefaults,
    pub(crate) admin: Option<AdminConfig>,
}

#[derive(Clone, Debug)]
pub(crate) struct AdminConfig {
    pub(crate) port: u16,
    /// The origin the panel is reached at, e.g.
    /// `https://gameservers-admin.grizzly-endeavors.com`. Compared against a
    /// write's `Origin` header, so a form on another site can't act through a
    /// signed-in admin's browser. Defaults to the panel host under the service
    /// domain, which is where the chart puts it.
    pub(crate) public_origin: String,
    pub(crate) required_group: String,
}

fn admin_config_from_env(lookup: EnvLookup) -> Result<Option<AdminConfig>> {
    let Some(port) = optional_port(lookup, "GAMESERVERS_ADMIN_PORT")? else {
        return Ok(None);
    };
    Ok(Some(AdminConfig {
        port,
        public_origin: optional(lookup, "GAMESERVERS_ADMIN_PUBLIC_URL")
            .map(|raw| raw.trim().trim_end_matches('/').to_owned())
            .filter(|origin| !origin.is_empty())
            .unwrap_or_else(|| DEFAULT_ADMIN_ORIGIN.to_owned()),
        required_group: optional(lookup, "GAMESERVERS_ADMIN_GROUP")
            .filter(|group| !group.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ADMIN_GROUP.to_owned()),
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GuildLimitDefaults {
    pub(crate) servers: u32,
    pub(crate) cpu_limit_millicores: i64,
    pub(crate) mem_limit_bytes: i64,
}

/// SMTP submission settings for transactional mail. Only the password is secret
/// (synced from the `platform-stalwart` 1Password item via ESO); the rest carry
/// infra defaults.
///
/// The host is deliberately the **public** name even though the bot runs in the
/// cluster: Stalwart's listeners trust PROXY-protocol from `10.0.0.0/8`, which
/// covers the pod network, so a direct in-cluster connection is read as a PROXY
/// header and reset. The public path rides the VPS `HAProxy`, which prepends the
/// header correctly. See grizzly-platform `docs/integration/mail.md`.
#[derive(Clone, Debug)]
pub(crate) struct MailConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) password: String,
    /// Must be `@grizzly-endeavors.com` or DMARC alignment fails and the mail is
    /// filtered rather than bounced — a silent failure, hence the startup check.
    pub(crate) from: String,
}

#[derive(Clone, Debug)]
pub(crate) struct LangfuseConfig {
    pub(crate) host: String,
    pub(crate) public_key: String,
    pub(crate) secret_key: String,
}

#[derive(Clone, Debug)]
pub(crate) struct S3Config {
    pub(crate) endpoint: String,
    pub(crate) bucket: String,
    pub(crate) region: String,
    pub(crate) access_key: String,
    pub(crate) secret_key: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ValkeyConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) db: u8,
    pub(crate) password: String,
}

impl ValkeyConfig {
    pub(crate) fn url(&self) -> String {
        format!(
            "redis://:{}@{}:{}/{}",
            self.password, self.host, self.port, self.db
        )
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DbConfig {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) database: String,
    pub(crate) user: String,
    pub(crate) password: String,
}

impl BotConfig {
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
        let guild_allowlist = parse_snowflakes(
            optional(lookup, "GAMESERVERS_GUILD_ALLOWLIST").as_deref(),
            "GAMESERVERS_GUILD_ALLOWLIST",
        )?;
        let throttle = throttle_policy_from_env(lookup)?;
        let beta_invite_url = optional(lookup, "GAMESERVERS_BETA_INVITE_URL").unwrap_or_default();
        let community_url = optional(lookup, "GAMESERVERS_COMMUNITY_URL").unwrap_or_default();

        let ollama_api_key = optional(lookup, "OLLAMA_API_KEY").filter(|key| !key.is_empty());
        let ollama_base_url = optional(lookup, "OLLAMA_BASE_URL")
            .unwrap_or_else(|| DEFAULT_OLLAMA_BASE_URL.to_owned());
        let ollama_model =
            optional(lookup, "OLLAMA_MODEL").unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned());

        let db = db_config_from_env(lookup)?;
        let valkey = valkey_config_from_env(lookup)?;
        let supervisor_token =
            optional(lookup, "SUPERVISOR_CONTROL_TOKEN").filter(|secret| !secret.trim().is_empty());
        let s3 = s3_config_from_env(lookup);
        let mail = mail_config_from_env(lookup)?;
        let CycleCadences {
            backup_interval,
            backup_retention,
            archive_retention_days,
            reconcile_interval,
            stranded_dwell,
            sample_interval,
        } = cycle_cadences_from_env(lookup)?;
        let agent_port =
            optional_port(lookup, "GAMESERVERS_AGENT_PORT")?.unwrap_or(DEFAULT_AGENT_PORT);
        let EndpointCredentials {
            ingame: ingame_token,
            operator: operator_token,
            signup: signup_token,
        } = endpoint_credentials_from_env(lookup);
        let environment = environment_from_env(lookup);
        let langfuse = langfuse_config_from_env(lookup);
        let release =
            optional(lookup, "GAMESERVERS_RELEASE").unwrap_or_else(|| DEFAULT_RELEASE.to_owned());
        let watchdog = watchdog_mode_from_env(lookup)?;
        let guild_limits = guild_limit_defaults_from_env(lookup)?;
        let admin = admin_config_from_env(lookup)?;

        Ok(Self {
            token,
            namespace,
            domain,
            catalog_dir,
            control_port,
            operator_ids,
            guild_allowlist,
            beta_invite_url,
            community_url,
            throttle,
            ollama_api_key,
            ollama_base_url,
            ollama_model,
            db,
            valkey,
            supervisor_token,
            s3,
            mail,
            backup_interval,
            backup_retention,
            archive_retention_days,
            reconcile_interval,
            stranded_dwell,
            sample_interval,
            agent_port,
            ingame_token,
            operator_token,
            signup_token,
            environment,
            langfuse,
            release,
            watchdog,
            guild_limits,
            admin,
        })
    }
}

fn guild_limit_defaults_from_env(lookup: EnvLookup) -> Result<GuildLimitDefaults> {
    let servers = optional_positive_u64(lookup, "GAMESERVERS_GUILD_MAX_SERVERS")?.map_or(
        Ok(DEFAULT_GUILD_MAX_SERVERS),
        |value| {
            u32::try_from(value).with_context(|| {
                format!("GAMESERVERS_GUILD_MAX_SERVERS must fit a u32, got {value}")
            })
        },
    )?;
    let cpu_limit_millicores = optional_quantity(
        lookup,
        "GAMESERVERS_GUILD_MAX_CPU",
        parse_cpu_millicores,
        "a CPU quantity like \"24\" or \"24000m\"",
    )?
    .unwrap_or(DEFAULT_GUILD_MAX_CPU_MILLICORES);
    let mem_limit_bytes = optional_quantity(
        lookup,
        "GAMESERVERS_GUILD_MAX_MEMORY",
        parse_bytes,
        "a memory quantity like \"48Gi\"",
    )?
    .unwrap_or(DEFAULT_GUILD_MAX_MEM_BYTES);
    Ok(GuildLimitDefaults {
        servers,
        cpu_limit_millicores,
        mem_limit_bytes,
    })
}

fn optional_quantity(
    lookup: EnvLookup,
    key: &str,
    parse: fn(&str) -> Option<i64>,
    expected: &str,
) -> Result<Option<i64>> {
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    let value = parse(raw.trim())
        .filter(|&value| value > 0)
        .with_context(|| format!("{key} must be {expected}, got {raw:?}"))?;
    Ok(Some(value))
}

fn watchdog_mode_from_env(lookup: EnvLookup) -> Result<WatchdogMode> {
    let Some(raw) = optional(lookup, "GAMESERVERS_WATCHDOG") else {
        return Ok(WatchdogMode::On);
    };
    parse_watchdog_mode(&raw)
        .with_context(|| format!("GAMESERVERS_WATCHDOG must be one of on/shadow/off, got {raw:?}"))
}

fn environment_from_env(lookup: EnvLookup) -> Environment {
    if lookup(IN_CLUSTER_MARKER).is_some() {
        Environment::Production
    } else {
        Environment::Development
    }
}

fn langfuse_config_from_env(lookup: EnvLookup) -> Option<LangfuseConfig> {
    let public_key = optional(lookup, "LANGFUSE_PUBLIC_KEY").filter(|value| !value.is_empty());
    let secret_key = optional(lookup, "LANGFUSE_SECRET_KEY").filter(|value| !value.is_empty());
    let (public_key, secret_key) = (public_key?, secret_key?);
    Some(LangfuseConfig {
        host: optional(lookup, "LANGFUSE_HOST").unwrap_or_else(|| DEFAULT_LANGFUSE_HOST.to_owned()),
        public_key,
        secret_key,
    })
}

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

struct CycleCadences {
    backup_interval: Duration,
    backup_retention: usize,
    archive_retention_days: i32,
    reconcile_interval: Duration,
    stranded_dwell: Duration,
    sample_interval: Duration,
}

fn cycle_cadences_from_env(lookup: EnvLookup) -> Result<CycleCadences> {
    let minutes = |key: &str, fallback: u64| -> Result<Duration> {
        Ok(Duration::from_secs(
            optional_positive_u64(lookup, key)?
                .unwrap_or(fallback)
                .saturating_mul(60),
        ))
    };
    let archive_retention_days =
        optional_positive_u64(lookup, "GAMESERVERS_ARCHIVE_RETENTION_DAYS")?
            .unwrap_or(DEFAULT_ARCHIVE_RETENTION_DAYS);
    Ok(CycleCadences {
        backup_interval: Duration::from_secs(
            optional_positive_u64(lookup, "GAMESERVERS_BACKUP_INTERVAL_HOURS")?
                .unwrap_or(DEFAULT_BACKUP_INTERVAL_HOURS)
                .saturating_mul(3600),
        ),
        backup_retention: optional_positive_usize(lookup, "GAMESERVERS_BACKUP_RETENTION")?
            .unwrap_or(DEFAULT_BACKUP_RETENTION),
        archive_retention_days: i32::try_from(archive_retention_days).unwrap_or(i32::MAX),
        reconcile_interval: minutes(
            "GAMESERVERS_RECONCILE_INTERVAL_MINUTES",
            DEFAULT_RECONCILE_INTERVAL_MINUTES,
        )?,
        stranded_dwell: minutes(
            "GAMESERVERS_STRANDED_DWELL_MINUTES",
            DEFAULT_STRANDED_DWELL_MINUTES,
        )?,
        sample_interval: minutes(
            "GAMESERVERS_SAMPLE_INTERVAL_MINUTES",
            DEFAULT_SAMPLE_INTERVAL_MINUTES,
        )?,
    })
}

struct EndpointCredentials {
    ingame: Option<String>,
    operator: Option<String>,
    signup: Option<String>,
}

fn endpoint_credentials_from_env(lookup: EnvLookup) -> EndpointCredentials {
    let read = |key: &str| optional(lookup, key).filter(|value| !value.trim().is_empty());
    EndpointCredentials {
        ingame: read("GAMESERVERS_INGAME_TOKEN"),
        operator: read("GAMESERVERS_OPERATOR_TOKEN"),
        signup: read("GAMESERVERS_SIGNUP_TOKEN"),
    }
}

fn mail_config_from_env(lookup: EnvLookup) -> Result<Option<MailConfig>> {
    let Some(password) =
        optional(lookup, "GAMESERVERS_SMTP_PASSWORD").filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let user =
        optional(lookup, "GAMESERVERS_SMTP_USER").unwrap_or_else(|| DEFAULT_SMTP_USER.to_owned());
    Ok(Some(MailConfig {
        host: optional(lookup, "GAMESERVERS_SMTP_HOST")
            .unwrap_or_else(|| DEFAULT_SMTP_HOST.to_owned()),
        port: optional_port(lookup, "GAMESERVERS_SMTP_PORT")?.unwrap_or(DEFAULT_SMTP_PORT),
        from: optional(lookup, "GAMESERVERS_MAIL_FROM").unwrap_or_else(|| user.clone()),
        user,
        password,
    }))
}

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

fn parse_user_ids(raw: Option<&str>) -> Result<Vec<u64>> {
    parse_snowflakes(raw, "GAMESERVERS_ADMIN_USER_IDS")
}

fn parse_snowflakes(raw: Option<&str>, key: &str) -> Result<Vec<u64>> {
    raw.map_or_else(|| Ok(Vec::new()), |raw| parse_snowflake_list(raw, key))
}

pub(crate) fn parse_snowflake_list(raw: &str, label: &str) -> Result<Vec<u64>> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            entry
                .parse::<u64>()
                .with_context(|| format!("{label} contains a non-integer: {entry:?}"))
        })
        .collect()
}

fn throttle_policy_from_env(lookup: EnvLookup) -> Result<crate::throttle::ThrottlePolicy> {
    let burst = optional_number::<u32>(lookup, "GAMESERVERS_TURN_BURST")?
        .filter(|&burst| burst > 0)
        .unwrap_or(crate::throttle::DEFAULT_TURN_BURST);
    let refill_seconds = optional_number::<u64>(lookup, "GAMESERVERS_TURN_REFILL_SECONDS")?
        .filter(|&seconds| seconds > 0)
        .unwrap_or(crate::throttle::DEFAULT_TURN_REFILL_SECONDS);
    let max_concurrent = optional_number::<usize>(lookup, "GAMESERVERS_MAX_CONCURRENT_TURNS")?
        .filter(|&permits| permits > 0)
        .unwrap_or(crate::throttle::DEFAULT_MAX_CONCURRENT_TURNS);
    Ok(crate::throttle::ThrottlePolicy {
        burst,
        refill_interval: std::time::Duration::from_secs(refill_seconds),
        max_concurrent,
    })
}

fn optional_number<T>(lookup: EnvLookup, key: &str) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let Some(raw) = optional(lookup, key) else {
        return Ok(None);
    };
    raw.trim()
        .parse::<T>()
        .map(Some)
        .map_err(|err| anyhow!("{key} must be a number, got {raw:?}: {err}"))
}

fn required(lookup: EnvLookup, key: &str) -> Result<String> {
    let raw = lookup(key).with_context(|| format!("{key} is required but not set"))?;
    raw.into_string()
        .map_err(|bad| anyhow!("{key} is not valid UTF-8: {}", bad.display()))
}

fn optional(lookup: EnvLookup, key: &str) -> Option<String> {
    match lookup(key)?.into_string() {
        Ok(value) => Some(value),
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
