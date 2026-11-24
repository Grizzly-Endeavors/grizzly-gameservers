use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use arc_swap::ArcSwap;
use sqlx::postgres::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::{
    BotConfig, DbConfig, GuildLimitDefaults, WatchdogMode, parse_snowflake_list,
    parse_watchdog_mode,
};
use crate::events::{parse_bytes, parse_cpu_millicores};
use crate::sizing::SizingBounds;
use crate::store::connect_pool;
use crate::throttle::ThrottlePolicy;

const SETTINGS_POOL_MAX_CONNECTIONS: u32 = 2;

const SETTINGS_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS runtime_settings (\
        key TEXT PRIMARY KEY, \
        value TEXT NOT NULL, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Section {
    Agent,
    Throttle,
    Guilds,
    Signups,
    Limits,
    Sizing,
    Watchdog,
    Cycles,
}

impl Section {
    pub(crate) const ALL: &'static [Self] = &[
        Self::Agent,
        Self::Throttle,
        Self::Guilds,
        Self::Signups,
        Self::Limits,
        Self::Sizing,
        Self::Watchdog,
        Self::Cycles,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Agent => "Gary",
            Self::Throttle => "Turn admission",
            Self::Guilds => "Guild admission",
            Self::Signups => "Beta signups",
            Self::Limits => "Per-guild caps",
            Self::Sizing => "Per-server bounds",
            Self::Watchdog => "Watchdog",
            Self::Cycles => "Background cycles",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reload {
    Hot,
    Restart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SettingKey {
    GaryModel,
    GaryBaseUrl,
    GaryMaxRounds,
    TranscriptTriggerTokens,
    TranscriptTargetTokens,
    ThrottleBurst,
    ThrottleRefillSeconds,
    ThrottleMaxConcurrent,
    GuildAllowlist,
    BetaInviteUrl,
    CommunityUrl,
    GuildMaxServers,
    GuildMaxCpu,
    GuildMaxMemory,
    SizingMinCpu,
    SizingMinMemory,
    SizingMaxCpu,
    SizingMaxMemory,
    WatchdogModeKey,
    BackupIntervalHours,
    BackupRetention,
    ArchiveRetentionDays,
    ReconcileIntervalMinutes,
    StrandedDwellMinutes,
    SampleIntervalMinutes,
}

impl SettingKey {
    pub(crate) const ALL: &'static [Self] = &[
        Self::GaryModel,
        Self::GaryBaseUrl,
        Self::GaryMaxRounds,
        Self::TranscriptTriggerTokens,
        Self::TranscriptTargetTokens,
        Self::ThrottleBurst,
        Self::ThrottleRefillSeconds,
        Self::ThrottleMaxConcurrent,
        Self::GuildAllowlist,
        Self::BetaInviteUrl,
        Self::CommunityUrl,
        Self::GuildMaxServers,
        Self::GuildMaxCpu,
        Self::GuildMaxMemory,
        Self::SizingMinCpu,
        Self::SizingMinMemory,
        Self::SizingMaxCpu,
        Self::SizingMaxMemory,
        Self::WatchdogModeKey,
        Self::BackupIntervalHours,
        Self::BackupRetention,
        Self::ArchiveRetentionDays,
        Self::ReconcileIntervalMinutes,
        Self::StrandedDwellMinutes,
        Self::SampleIntervalMinutes,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::GaryModel => "gary.model",
            Self::GaryBaseUrl => "gary.base_url",
            Self::GaryMaxRounds => "gary.max_rounds",
            Self::TranscriptTriggerTokens => "gary.transcript_trigger_tokens",
            Self::TranscriptTargetTokens => "gary.transcript_target_tokens",
            Self::ThrottleBurst => "throttle.burst",
            Self::ThrottleRefillSeconds => "throttle.refill_seconds",
            Self::ThrottleMaxConcurrent => "throttle.max_concurrent",
            Self::GuildAllowlist => "guilds.allowlist",
            Self::BetaInviteUrl => "signups.invite_url",
            Self::CommunityUrl => "signups.community_url",
            Self::GuildMaxServers => "limits.guild_max_servers",
            Self::GuildMaxCpu => "limits.guild_max_cpu",
            Self::GuildMaxMemory => "limits.guild_max_memory",
            Self::SizingMinCpu => "sizing.min_cpu",
            Self::SizingMinMemory => "sizing.min_memory",
            Self::SizingMaxCpu => "sizing.max_cpu",
            Self::SizingMaxMemory => "sizing.max_memory",
            Self::WatchdogModeKey => "watchdog.mode",
            Self::BackupIntervalHours => "backup.interval_hours",
            Self::BackupRetention => "backup.retention",
            Self::ArchiveRetentionDays => "archive.retention_days",
            Self::ReconcileIntervalMinutes => "reconcile.interval_minutes",
            Self::StrandedDwellMinutes => "reconcile.stranded_dwell_minutes",
            Self::SampleIntervalMinutes => "sampling.interval_minutes",
        }
    }

    pub(crate) fn from_key(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|key| key.as_str() == raw)
    }

    pub(crate) fn section(self) -> Section {
        match self {
            Self::GaryModel
            | Self::GaryBaseUrl
            | Self::GaryMaxRounds
            | Self::TranscriptTriggerTokens
            | Self::TranscriptTargetTokens => Section::Agent,
            Self::ThrottleBurst | Self::ThrottleRefillSeconds | Self::ThrottleMaxConcurrent => {
                Section::Throttle
            }
            Self::GuildAllowlist => Section::Guilds,
            Self::BetaInviteUrl | Self::CommunityUrl => Section::Signups,
            Self::GuildMaxServers | Self::GuildMaxCpu | Self::GuildMaxMemory => Section::Limits,
            Self::SizingMinCpu
            | Self::SizingMinMemory
            | Self::SizingMaxCpu
            | Self::SizingMaxMemory => Section::Sizing,
            Self::WatchdogModeKey => Section::Watchdog,
            Self::BackupIntervalHours
            | Self::BackupRetention
            | Self::ArchiveRetentionDays
            | Self::ReconcileIntervalMinutes
            | Self::StrandedDwellMinutes
            | Self::SampleIntervalMinutes => Section::Cycles,
        }
    }

    pub(crate) fn reload(self) -> Reload {
        match self {
            Self::BackupIntervalHours
            | Self::BackupRetention
            | Self::ArchiveRetentionDays
            | Self::ReconcileIntervalMinutes
            | Self::StrandedDwellMinutes
            | Self::SampleIntervalMinutes => Reload::Restart,
            Self::GaryModel
            | Self::GaryBaseUrl
            | Self::GaryMaxRounds
            | Self::TranscriptTriggerTokens
            | Self::TranscriptTargetTokens
            | Self::ThrottleBurst
            | Self::ThrottleRefillSeconds
            | Self::ThrottleMaxConcurrent
            | Self::GuildAllowlist
            | Self::BetaInviteUrl
            | Self::CommunityUrl
            | Self::GuildMaxServers
            | Self::GuildMaxCpu
            | Self::GuildMaxMemory
            | Self::SizingMinCpu
            | Self::SizingMinMemory
            | Self::SizingMaxCpu
            | Self::SizingMaxMemory
            | Self::WatchdogModeKey => Reload::Hot,
        }
    }

    pub(crate) fn summary(self) -> &'static str {
        match self {
            Self::GaryModel => "Model tag Gary drives for every turn.",
            Self::GaryBaseUrl => "OpenAI-compatible chat-completions base URL.",
            Self::GaryMaxRounds => "Model calls one turn may spend before it escalates to a human.",
            Self::TranscriptTriggerTokens => {
                "Prompt tokens a conversation may reach before its oldest turns are dropped."
            }
            Self::TranscriptTargetTokens => {
                "Size a conversation is cut back to once it trips the trigger. Well below the \
                 trigger on purpose: trimming a little at a time would re-cost the whole prompt \
                 on every turn."
            }
            Self::ThrottleBurst => "Turns one person may start back-to-back.",
            Self::ThrottleRefillSeconds => "Seconds to earn one turn back.",
            Self::ThrottleMaxConcurrent => "Turns allowed to run at once across every surface.",
            Self::GuildAllowlist => {
                "Discord servers admitted, comma-separated. Empty serves every server the bot is \
                 invited to."
            }
            Self::BetaInviteUrl => {
                "Where an approved applicant is sent to add the bot. Carries the client id and \
                 permissions, so it changes whenever the bot needs a new scope."
            }
            Self::CommunityUrl => {
                "Community server link offered in the approval email. Leave it unset to omit \
                 that paragraph entirely."
            }
            Self::GuildMaxServers => "Game servers one Discord server may hold at once.",
            Self::GuildMaxCpu => "Total CPU one Discord server's live game servers may declare.",
            Self::GuildMaxMemory => {
                "Total memory one Discord server's live game servers may declare."
            }
            Self::SizingMinCpu => "Least CPU a single game server may be given.",
            Self::SizingMinMemory => "Least memory a single game server may be given.",
            Self::SizingMaxCpu => "Most CPU a single game server may be given.",
            Self::SizingMaxMemory => "Most memory a single game server may be given.",
            Self::WatchdogModeKey => {
                "on responds to incidents, shadow only records them, off disables detection."
            }
            Self::BackupIntervalHours => "How often every running server is backed up.",
            Self::BackupRetention => "Backups kept per server before older ones are pruned.",
            Self::ArchiveRetentionDays => "How long an archived world is kept.",
            Self::ReconcileIntervalMinutes => "How often stranded servers are swept for.",
            Self::StrandedDwellMinutes => {
                "How long a pod must sit terminating before it is forced out."
            }
            Self::SampleIntervalMinutes => "How often occupancy and resource samples are taken.",
        }
    }

    pub(crate) fn render(self, snapshot: &SettingsSnapshot) -> String {
        match self {
            Self::GaryModel => snapshot.gary_model.clone(),
            Self::GaryBaseUrl => snapshot.gary_base_url.clone(),
            Self::GaryMaxRounds => snapshot.gary_max_rounds.to_string(),
            Self::TranscriptTriggerTokens => snapshot.transcript_trigger_tokens.to_string(),
            Self::TranscriptTargetTokens => snapshot.transcript_target_tokens.to_string(),
            Self::ThrottleBurst => snapshot.throttle.burst.to_string(),
            Self::ThrottleRefillSeconds => snapshot.throttle.refill_interval.as_secs().to_string(),
            Self::ThrottleMaxConcurrent => snapshot.throttle.max_concurrent.to_string(),
            Self::GuildAllowlist => snapshot
                .guild_allowlist
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
            Self::BetaInviteUrl => snapshot.beta_invite_url.clone(),
            Self::CommunityUrl => snapshot.community_url.clone(),
            Self::GuildMaxServers => snapshot.guild_limits.servers.to_string(),
            Self::GuildMaxCpu => exact_cpu(snapshot.guild_limits.cpu_limit_millicores),
            Self::GuildMaxMemory => exact_bytes(snapshot.guild_limits.mem_limit_bytes),
            Self::SizingMinCpu => exact_cpu(snapshot.sizing_bounds.min_cpu_millicores),
            Self::SizingMinMemory => exact_bytes(snapshot.sizing_bounds.min_mem_bytes),
            Self::SizingMaxCpu => exact_cpu(snapshot.sizing_bounds.max_cpu_millicores),
            Self::SizingMaxMemory => exact_bytes(snapshot.sizing_bounds.max_mem_bytes),
            Self::WatchdogModeKey => snapshot.watchdog.as_str().to_owned(),
            Self::BackupIntervalHours => (snapshot.backup_interval.as_secs() / 3600).to_string(),
            Self::BackupRetention => snapshot.backup_retention.to_string(),
            Self::ArchiveRetentionDays => snapshot.archive_retention_days.to_string(),
            Self::ReconcileIntervalMinutes => {
                (snapshot.reconcile_interval.as_secs() / 60).to_string()
            }
            Self::StrandedDwellMinutes => (snapshot.stranded_dwell.as_secs() / 60).to_string(),
            Self::SampleIntervalMinutes => (snapshot.sample_interval.as_secs() / 60).to_string(),
        }
    }

    pub(crate) fn expected(self) -> &'static str {
        match self {
            Self::GaryModel => "a model tag like \"glm-5.2\"",
            Self::GaryBaseUrl => "a base URL like \"https://ollama.com/v1\"",
            Self::GaryMaxRounds
            | Self::TranscriptTriggerTokens
            | Self::TranscriptTargetTokens
            | Self::ThrottleBurst
            | Self::ThrottleRefillSeconds
            | Self::ThrottleMaxConcurrent
            | Self::GuildMaxServers
            | Self::BackupIntervalHours
            | Self::BackupRetention
            | Self::ArchiveRetentionDays
            | Self::ReconcileIntervalMinutes
            | Self::StrandedDwellMinutes
            | Self::SampleIntervalMinutes => "a whole number of 1 or more",
            Self::GuildAllowlist => "comma-separated Discord server ids, or nothing at all",
            Self::BetaInviteUrl | Self::CommunityUrl => "an https:// URL",
            Self::GuildMaxCpu | Self::SizingMinCpu | Self::SizingMaxCpu => {
                "a CPU amount like \"24\" or \"24000m\""
            }
            Self::GuildMaxMemory | Self::SizingMinMemory | Self::SizingMaxMemory => {
                "a memory amount like \"48Gi\""
            }
            Self::WatchdogModeKey => "one of on, shadow, or off",
        }
    }

    fn assign(self, snapshot: &mut SettingsSnapshot, raw: &str) -> Result<()> {
        let expected = self.expected();
        match self {
            Self::GaryModel => snapshot.gary_model = text(raw, expected)?,
            Self::GaryBaseUrl => snapshot.gary_base_url = text(raw, expected)?,
            Self::GaryMaxRounds => snapshot.gary_max_rounds = count(raw, expected)?,
            Self::TranscriptTriggerTokens => {
                snapshot.transcript_trigger_tokens = count(raw, expected)?;
            }
            Self::TranscriptTargetTokens => {
                snapshot.transcript_target_tokens = count(raw, expected)?;
            }
            Self::ThrottleBurst => {
                snapshot.throttle.burst = u32::try_from(count(raw, expected)?).unwrap_or(u32::MAX);
            }
            Self::ThrottleRefillSeconds => {
                snapshot.throttle.refill_interval = Duration::from_secs(seconds(raw, expected)?);
            }
            Self::ThrottleMaxConcurrent => snapshot.throttle.max_concurrent = count(raw, expected)?,
            Self::GuildAllowlist => {
                snapshot.guild_allowlist = parse_snowflake_list(raw, "the allowlist")?.into();
            }
            Self::BetaInviteUrl => snapshot.beta_invite_url = parse_https_url(raw, "the invite")?,
            Self::CommunityUrl => {
                snapshot.community_url = parse_https_url(raw, "the community link")?;
            }
            Self::GuildMaxServers => {
                snapshot.guild_limits.servers =
                    u32::try_from(count(raw, expected)?).unwrap_or(u32::MAX);
            }
            Self::GuildMaxCpu => {
                snapshot.guild_limits.cpu_limit_millicores = cpu(raw, expected)?;
            }
            Self::GuildMaxMemory => snapshot.guild_limits.mem_limit_bytes = memory(raw, expected)?,
            Self::SizingMinCpu => snapshot.sizing_bounds.min_cpu_millicores = cpu(raw, expected)?,
            Self::SizingMinMemory => snapshot.sizing_bounds.min_mem_bytes = memory(raw, expected)?,
            Self::SizingMaxCpu => snapshot.sizing_bounds.max_cpu_millicores = cpu(raw, expected)?,
            Self::SizingMaxMemory => snapshot.sizing_bounds.max_mem_bytes = memory(raw, expected)?,
            Self::WatchdogModeKey => {
                snapshot.watchdog = parse_watchdog_mode(raw)
                    .with_context(|| format!("needs {expected}, got {raw:?}"))?;
            }
            Self::BackupIntervalHours => {
                snapshot.backup_interval = Duration::from_secs(seconds(raw, expected)? * 3600);
            }
            Self::BackupRetention => snapshot.backup_retention = count(raw, expected)?,
            Self::ArchiveRetentionDays => {
                snapshot.archive_retention_days =
                    i32::try_from(count(raw, expected)?).unwrap_or(i32::MAX);
            }
            Self::ReconcileIntervalMinutes => {
                snapshot.reconcile_interval = Duration::from_secs(seconds(raw, expected)? * 60);
            }
            Self::StrandedDwellMinutes => {
                snapshot.stranded_dwell = Duration::from_secs(seconds(raw, expected)? * 60);
            }
            Self::SampleIntervalMinutes => {
                snapshot.sample_interval = Duration::from_secs(seconds(raw, expected)? * 60);
            }
        }
        Ok(())
    }
}

fn text(raw: &str, expected: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("needs {expected}, got an empty value");
    }
    Ok(trimmed.to_owned())
}

fn count(raw: &str, expected: &str) -> Result<usize> {
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

fn seconds(raw: &str, expected: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

fn cpu(raw: &str, expected: &str) -> Result<i64> {
    parse_cpu_millicores(raw.trim())
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

fn memory(raw: &str, expected: &str) -> Result<i64> {
    parse_bytes(raw.trim())
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsSnapshot {
    pub(crate) gary_model: String,
    pub(crate) gary_base_url: String,
    pub(crate) gary_max_rounds: usize,
    pub(crate) transcript_trigger_tokens: usize,
    pub(crate) transcript_target_tokens: usize,
    pub(crate) throttle: ThrottlePolicy,
    pub(crate) guild_allowlist: Arc<[u64]>,
    pub(crate) beta_invite_url: String,
    pub(crate) community_url: String,
    pub(crate) guild_limits: GuildLimitDefaults,
    pub(crate) sizing_bounds: SizingBounds,
    pub(crate) watchdog: WatchdogMode,
    pub(crate) backup_interval: Duration,
    pub(crate) backup_retention: usize,
    pub(crate) archive_retention_days: i32,
    pub(crate) reconcile_interval: Duration,
    pub(crate) stranded_dwell: Duration,
    pub(crate) sample_interval: Duration,
}

fn parse_https_url(raw: &str, subject: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let parsed = url::Url::parse(trimmed)
        .with_context(|| format!("the {subject} link isn't a URL: {trimmed}"))?;
    if parsed.scheme() != "https" {
        return Err(anyhow!("the {subject} link must start with https://"));
    }
    Ok(trimmed.to_owned())
}

impl SettingsSnapshot {
    pub(crate) fn baseline(config: &BotConfig) -> Self {
        Self {
            gary_model: config.ollama_model.clone(),
            gary_base_url: config.ollama_base_url.clone(),
            gary_max_rounds: crate::agent::DEFAULT_MAX_ROUNDS,
            transcript_trigger_tokens: crate::agent::DEFAULT_TRANSCRIPT_TRIGGER_TOKENS,
            transcript_target_tokens: crate::agent::DEFAULT_TRANSCRIPT_TARGET_TOKENS,
            throttle: config.throttle,
            guild_allowlist: config.guild_allowlist.clone().into(),
            beta_invite_url: config.beta_invite_url.clone(),
            community_url: config.community_url.clone(),
            guild_limits: config.guild_limits,
            sizing_bounds: SizingBounds::default(),
            watchdog: config.watchdog,
            backup_interval: config.backup_interval,
            backup_retention: config.backup_retention,
            archive_retention_days: config.archive_retention_days,
            reconcile_interval: config.reconcile_interval,
            stranded_dwell: config.stranded_dwell,
            sample_interval: config.sample_interval,
        }
    }

    pub(crate) fn transcript_limits(&self) -> crate::agent::TranscriptLimits {
        crate::agent::TranscriptLimits {
            trigger_tokens: self.transcript_trigger_tokens,
            target_tokens: self.transcript_target_tokens,
        }
    }

    fn coherence(&self) -> Result<()> {
        if self.transcript_target_tokens >= self.transcript_trigger_tokens {
            bail!(
                "a conversation would be cut back to the same size that triggers the cut, so \
                 every turn after the first would pay to re-send the whole conversation — set \
                 the target below the trigger"
            );
        }
        if !self.sizing_bounds.is_coherent() {
            bail!(
                "the smallest a server may be would end up larger than the largest it may be — \
                 raise the maximum first, or lower the minimum"
            );
        }
        Ok(())
    }

    fn coherence_checked(self) -> Result<Self> {
        self.coherence()?;
        Ok(self)
    }

    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self {
            gary_model: "glm-5.2".to_owned(),
            gary_base_url: "https://ollama.com/v1".to_owned(),
            gary_max_rounds: crate::agent::DEFAULT_MAX_ROUNDS,
            transcript_trigger_tokens: crate::agent::DEFAULT_TRANSCRIPT_TRIGGER_TOKENS,
            transcript_target_tokens: crate::agent::DEFAULT_TRANSCRIPT_TARGET_TOKENS,
            throttle: ThrottlePolicy::default(),
            guild_allowlist: Vec::new().into(),
            beta_invite_url: String::new(),
            community_url: String::new(),
            guild_limits: GuildLimitDefaults {
                servers: 5,
                cpu_limit_millicores: 24_000,
                mem_limit_bytes: 48 * 1024 * 1024 * 1024,
            },
            sizing_bounds: SizingBounds::default(),
            watchdog: WatchdogMode::On,
            backup_interval: Duration::from_hours(24),
            backup_retention: 7,
            archive_retention_days: 90,
            reconcile_interval: Duration::from_mins(5),
            stranded_dwell: Duration::from_mins(15),
            sample_interval: Duration::from_mins(5),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredOverride {
    pub(crate) value: String,
    pub(crate) updated_at: String,
    pub(crate) updated_by: Option<String>,
}

type SettingRow = (String, String, String, Option<String>);

struct SettingsStore {
    pool: PgPool,
}

impl SettingsStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, SETTINGS_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(SETTINGS_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply runtime_settings schema")?;
        Ok(Self { pool })
    }

    async fn upsert(&self, key: SettingKey, value: &str, actor: &str) -> Result<String> {
        let (updated_at,): (String,) = sqlx::query_as(
            "INSERT INTO runtime_settings (key, value, updated_by) VALUES ($1, $2, $3) \
             ON CONFLICT (key) DO UPDATE SET \
             value = EXCLUDED.value, updated_at = now(), updated_by = EXCLUDED.updated_by \
             RETURNING updated_at::text",
        )
        .bind(key.as_str())
        .bind(value)
        .bind(actor)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("failed to save the {} setting", key.as_str()))?;
        Ok(updated_at)
    }

    async fn delete(&self, key: SettingKey) -> Result<()> {
        sqlx::query("DELETE FROM runtime_settings WHERE key = $1")
            .bind(key.as_str())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to clear the {} setting", key.as_str()))?;
        Ok(())
    }

    async fn load_all(&self) -> Result<BTreeMap<SettingKey, StoredOverride>> {
        let rows: Vec<SettingRow> =
            sqlx::query_as("SELECT key, value, updated_at::text, updated_by FROM runtime_settings")
                .fetch_all(&self.pool)
                .await
                .context("failed to load runtime settings")?;
        Ok(rows
            .into_iter()
            .filter_map(|(key, value, updated_at, updated_by)| {
                let Some(parsed) = SettingKey::from_key(&key) else {
                    warn!(key = %key, "ignoring an unrecognized runtime setting");
                    return None;
                };
                Some((
                    parsed,
                    StoredOverride {
                        value,
                        updated_at,
                        updated_by,
                    },
                ))
            })
            .collect())
    }
}

fn compose(
    baseline: &SettingsSnapshot,
    overrides: &BTreeMap<SettingKey, StoredOverride>,
) -> SettingsSnapshot {
    let mut snapshot = baseline.clone();
    for (key, stored) in overrides {
        if let Err(err) = key.assign(&mut snapshot, &stored.value) {
            error!(
                key = key.as_str(),
                value = %stored.value,
                error = ?err,
                "ignoring an unusable stored setting; falling back to the deployed value"
            );
        }
    }
    snapshot
}

fn exact_cpu(millicores: i64) -> String {
    format!("{millicores}m")
}

fn exact_bytes(bytes: i64) -> String {
    if bytes <= 0 {
        return bytes.to_string();
    }
    for (scale, suffix) in [
        (1_i64 << 30, "Gi"),
        (1_i64 << 20, "Mi"),
        (1_i64 << 10, "Ki"),
    ] {
        if bytes % scale == 0 {
            return format!("{}{suffix}", bytes / scale);
        }
    }
    bytes.to_string()
}

pub(crate) struct RuntimeSettings {
    store: Option<SettingsStore>,
    baseline: SettingsSnapshot,
    booted: Arc<SettingsSnapshot>,
    current: ArcSwap<SettingsSnapshot>,
    overrides: RwLock<BTreeMap<SettingKey, StoredOverride>>,
}

impl RuntimeSettings {
    pub(crate) async fn connect(db: Option<&DbConfig>, baseline: SettingsSnapshot) -> Self {
        let Some(db) = db else {
            return Self::fixed(baseline);
        };
        let store = match SettingsStore::connect(db).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; runtime setting overrides disabled");
                return Self::fixed(baseline);
            }
        };
        let overrides = match store.load_all().await {
            Ok(overrides) => overrides,
            Err(err) => {
                error!(error = ?err, "failed to load runtime settings; overrides disabled");
                return Self::fixed(baseline);
            }
        };
        let resolved = match compose(&baseline, &overrides).coherence_checked() {
            Ok(snapshot) => snapshot,
            Err(err) => {
                error!(
                    error = ?err,
                    "stored runtime settings are inconsistent; running on the deployed values"
                );
                baseline.clone()
            }
        };
        info!(
            overrides = overrides.len(),
            "loaded runtime setting overrides"
        );
        let resolved = Arc::new(resolved);
        Self {
            store: Some(store),
            baseline,
            booted: Arc::clone(&resolved),
            current: ArcSwap::new(resolved),
            overrides: RwLock::new(overrides),
        }
    }

    pub(crate) fn fixed(snapshot: SettingsSnapshot) -> Self {
        let resolved = Arc::new(snapshot.clone());
        Self {
            store: None,
            baseline: snapshot,
            booted: Arc::clone(&resolved),
            current: ArcSwap::new(resolved),
            overrides: RwLock::new(BTreeMap::new()),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<SettingsSnapshot> {
        self.current.load_full()
    }

    pub(crate) fn booted(&self) -> Arc<SettingsSnapshot> {
        Arc::clone(&self.booted)
    }

    pub(crate) fn baseline(&self) -> &SettingsSnapshot {
        &self.baseline
    }

    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) async fn overrides(&self) -> BTreeMap<SettingKey, StoredOverride> {
        self.overrides.read().await.clone()
    }

    pub(crate) async fn apply(
        &self,
        key: SettingKey,
        raw: Option<&str>,
        actor: &str,
    ) -> Result<Arc<SettingsSnapshot>> {
        let store = self
            .store
            .as_ref()
            .context("settings can't be changed right now — there's no database connection")?;
        let mut overrides = self.overrides.write().await;
        let mut candidate = overrides.clone();
        match raw {
            Some(raw) => {
                let mut probe = self.baseline.clone();
                key.assign(&mut probe, raw)?;
                candidate.insert(
                    key,
                    StoredOverride {
                        value: raw.trim().to_owned(),
                        updated_at: String::new(),
                        updated_by: Some(actor.to_owned()),
                    },
                );
            }
            None => {
                candidate.remove(&key);
            }
        }
        let next = compose(&self.baseline, &candidate).coherence_checked()?;

        match raw {
            Some(raw) => {
                let stamped = store.upsert(key, raw.trim(), actor).await?;
                if let Some(entry) = candidate.get_mut(&key) {
                    entry.updated_at = stamped;
                }
            }
            None => store.delete(key).await?,
        }

        let next = Arc::new(next);
        self.current.store(Arc::clone(&next));
        *overrides = candidate;
        info!(
            key = key.as_str(),
            actor,
            cleared = raw.is_none(),
            "runtime setting changed"
        );
        Ok(next)
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
