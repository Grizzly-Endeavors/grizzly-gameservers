//! Operator-tunable runtime settings: the knobs that used to need a redeploy.
//!
//! Every value here already had a home in [`BotConfig`](crate::config::BotConfig)
//! — an environment variable with a compiled default behind it. That worked while
//! the only person changing them was also the person rolling the deploy, but
//! during the beta the interesting knobs (which guilds are served, which model
//! Gary drives, how much of the cluster one guild may hold) turn out to want
//! changing on a timescale shorter than an image build. This module adds a third,
//! highest-precedence layer in front of the existing two:
//!
//! ```text
//! database override  >  environment variable  >  compiled default
//! ```
//!
//! Env is therefore demoted to a *deploy-time default* rather than the last word.
//! Nothing about the environment path changes: a deploy with no overrides behaves
//! exactly as it did before this module existed, and clearing an override returns
//! a setting to whatever the environment says.
//!
//! **Reads go through a whole-snapshot swap.** [`RuntimeSettings::snapshot`]
//! hands back an [`Arc<SettingsSnapshot>`] that is replaced as a unit, never
//! mutated in place, so a reader can't observe half of a multi-field change and a
//! long-running turn can pin one coherent view of the world for its whole
//! lifetime. Consumers read the snapshot at the point of use rather than keeping
//! a copy — a stored copy is how a hot setting quietly stops being hot.
//!
//! **Not everything can be hot.** The background cycles bake their cadence into a
//! ticker when their task spawns, so an override to one of those is in force from
//! the next boot rather than immediately. [`RuntimeSettings::booted`] keeps what
//! this process actually started with, so the panel can show "configured 12h,
//! still running the 24h it booted with" rather than leaving the operator to
//! remember.
//!
//! **Secrets are deliberately absent.** `OLLAMA_API_KEY`, the database password,
//! the bearer tokens — none of them are settings. They live in 1Password and
//! arrive through ESO, and rotating one is legitimately a redeploy. A `TEXT`
//! column an admin page can echo is the wrong place for a credential, so whether
//! Gary is configured *at all* stays a startup fact; only which model he drives
//! is tunable.
//!
//! Persistence follows the [`crate::limits`] shape: connecting never fails, and a
//! database that is absent or unreachable leaves the façade serving the baseline.
//! A settings store that can't be read must not be able to un-cap or un-throttle
//! anything.

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

/// Pool size for the settings table. Read once at startup into the snapshot and
/// written only when the operator changes a knob, so one spare is ample.
const SETTINGS_POOL_MAX_CONNECTIONS: u32 = 2;

const SETTINGS_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS runtime_settings (\
        key TEXT PRIMARY KEY, \
        value TEXT NOT NULL, \
        updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        updated_by TEXT)";

/// Which group of the admin panel a setting belongs under.
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
    /// Every section, in the order the panel lists them.
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

/// When a change to a setting starts mattering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reload {
    /// Live: the next read of the snapshot sees it. A turn already in flight
    /// keeps the view it started with, by design.
    Hot,
    /// Recorded now, in force after the next restart — the consumer captured the
    /// value at startup (a ticker's period, most often) and has no re-read point.
    Restart,
}

/// One operator-tunable knob. The variants are the whole catalog: `as_str` is
/// what lands in the database, and every other method matches exhaustively, so
/// adding a variant without deciding its grammar, its section, or how it renders
/// is a compile error rather than a silently half-wired setting.
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
    /// Where an approved applicant is sent to install the bot.
    BetaInviteUrl,
    /// Optional community server link the approval email offers.
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
    /// Every key, in the order the admin panel lists them.
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

    /// The database key. Stable across releases — renaming one orphans its
    /// stored override, which reads as the setting silently reverting.
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

    /// Resolve a database key back to its variant, or `None` for a key this
    /// build doesn't know — a row left behind by an older or newer release.
    pub(crate) fn from_key(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|key| key.as_str() == raw)
    }

    /// Which group of the admin panel this key is listed under. Presentation
    /// only, but it lives beside the key so a new setting can't be added without
    /// deciding where a human would look for it.
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

    /// Whether a change is live or waits for the next restart. The cycle
    /// cadences are the restart set: each is captured into a `tokio` ticker when
    /// its task spawns, and re-reading it per tick would buy a few minutes'
    /// latency on a knob nobody turns in a hurry.
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

    /// One line describing what the knob does, shown beside its field.
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

    /// Render this key's value out of `snapshot` in a form [`Self::assign`]
    /// accepts back unchanged — the panel puts it straight into an editable
    /// field, so a round trip that isn't lossless would silently rewrite a
    /// setting the operator only opened to look at.
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

    /// The grammar this key accepts, phrased to complete "needs …". Used in the
    /// rejection message, so it has to describe a value the operator could type.
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

    /// Parse `raw` and write it into `snapshot`.
    ///
    /// # Errors
    ///
    /// Returns a plain-language rejection naming the value and the grammar it
    /// failed — this text reaches the operator in the panel, so it explains what
    /// to type rather than what failed internally.
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

/// A non-empty free-text value, trimmed.
fn text(raw: &str, expected: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("needs {expected}, got an empty value");
    }
    Ok(trimmed.to_owned())
}

/// A whole number of 1 or more, as a `usize`. Zero is rejected everywhere it
/// appears here: a zero retention prunes everything, a zero interval panics
/// `tokio::time::interval`, and a zero burst refuses every turn.
fn count(raw: &str, expected: &str) -> Result<usize> {
    raw.trim()
        .parse::<usize>()
        .ok()
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

/// A whole number of 1 or more, as a `u64` of seconds/hours/minutes.
fn seconds(raw: &str, expected: &str) -> Result<u64> {
    raw.trim()
        .parse::<u64>()
        .ok()
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

/// A positive Kubernetes CPU quantity, in millicores.
fn cpu(raw: &str, expected: &str) -> Result<i64> {
    parse_cpu_millicores(raw.trim())
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

/// A positive Kubernetes memory quantity, in bytes.
fn memory(raw: &str, expected: &str) -> Result<i64> {
    parse_bytes(raw.trim())
        .filter(|&value| value > 0)
        .with_context(|| format!("needs {expected}, got {raw:?}"))
}

/// Every tunable value, resolved. Replaced as a whole so readers see a coherent
/// set; cheap to clone because the only non-`Copy` fields are two short strings
/// and a shared slice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsSnapshot {
    pub(crate) gary_model: String,
    pub(crate) gary_base_url: String,
    pub(crate) gary_max_rounds: usize,
    /// Prompt tokens a transcript may reach before it is cut back.
    pub(crate) transcript_trigger_tokens: usize,
    /// Where a cut lands it. See [`SettingKey::TranscriptTargetTokens`].
    pub(crate) transcript_target_tokens: usize,
    pub(crate) throttle: ThrottlePolicy,
    /// Guilds admitted. Empty admits every guild — see
    /// [`BotConfig::guild_allowlist`](crate::config::BotConfig::guild_allowlist).
    pub(crate) guild_allowlist: Arc<[u64]>,
    /// Install link mailed to an approved applicant. Empty means approving is
    /// refused rather than sending mail with nowhere to point.
    pub(crate) beta_invite_url: String,
    /// Community server link the approval email offers. Empty omits it.
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

/// Parse an `https://` URL, returning it **exactly as typed** after trimming.
///
/// Deliberately does not return `Url::to_string()`: that normalizes a bare
/// authority by appending a trailing slash, so the panel would rewrite a setting
/// the operator only opened to look at. [`SettingKey::render`] has to round-trip
/// losslessly through [`SettingKey::assign`], so the parse is validation only and
/// the parsed value is discarded.
///
/// Empty is accepted and means "not set", the same way an empty guild allowlist
/// is meaningful rather than invalid — a fresh deploy has no invite link, and its
/// baseline has to survive the round trip. Refusing to *act* on an unset link is
/// the approve handler's job, where the consequence (mail pointing nowhere) is
/// actually visible.
///
/// # Errors
///
/// Returns an error naming `subject` if a non-empty value isn't a valid `https`
/// URL.
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
    /// The baseline: environment values where set, compiled defaults otherwise.
    /// This is what an override layers on top of, and what clearing one returns
    /// to — so a setting the operator never touched behaves exactly as it did
    /// before this module existed.
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

    /// The transcript bounds a `commit` trims between.
    pub(crate) fn transcript_limits(&self) -> crate::agent::TranscriptLimits {
        crate::agent::TranscriptLimits {
            trigger_tokens: self.transcript_trigger_tokens,
            target_tokens: self.transcript_target_tokens,
        }
    }

    /// Cross-field rules no single key can enforce alone.
    ///
    /// # Errors
    ///
    /// Returns a plain-language description of the inconsistency.
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

    /// `self`, or an error describing why it isn't internally consistent.
    ///
    /// # Errors
    ///
    /// Returns the first cross-field rule this snapshot violates.
    fn coherence_checked(self) -> Result<Self> {
        self.coherence()?;
        Ok(self)
    }

    /// The compiled defaults, without reading an environment. Lets a consumer's
    /// tests build a settings façade without standing up a whole `BotConfig`.
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

/// One stored override, with the audit trail the panel shows beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoredOverride {
    pub(crate) value: String,
    /// When the override was last written, as Postgres rendered it. Text rather
    /// than a timestamp type because `sqlx` here carries no date feature, which
    /// is the same reason `archives.created_at` is read as text.
    pub(crate) updated_at: String,
    pub(crate) updated_by: Option<String>,
}

/// One `runtime_settings` row as it comes off the wire.
type SettingRow = (String, String, String, Option<String>);

/// A connection pool for the settings table, schema applied.
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

    /// Write an override, returning the timestamp Postgres stamped on it — read
    /// back rather than guessed so the panel's "changed at" is the database's
    /// own answer, not this process's clock.
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
                    // A row this build has no variant for: an override written by
                    // a newer release, or one whose key was retired. Left in place
                    // rather than deleted, so a rollback doesn't lose it.
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

/// Layer the stored overrides onto the baseline.
///
/// Tolerant on purpose: a row that no longer parses — a bound tightened by a
/// release, a value hand-edited in `psql` — is logged and skipped rather than
/// failing the boot. Every write is validated before it lands, so reaching this
/// path at all means something bypassed the panel. Skipping is safe because
/// [`SettingKey::assign`] writes its field only after parsing succeeds, so a
/// rejected value leaves the snapshot exactly as it found it.
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

/// Millicores in the form the CPU parser reads back exactly. Always suffixed,
/// because a bare `24` means twenty-four *cores* to that parser and rendering
/// 24 millicores as `24` would multiply the value by a thousand on the next save.
fn exact_cpu(millicores: i64) -> String {
    format!("{millicores}m")
}

/// Bytes in the largest binary unit that divides them exactly, so `51539607552`
/// comes back as `48Gi` and still parses to the same number. Exactness is the
/// point: a rounded display value saved unchanged would quietly resize the cap.
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

/// The settings façade: baseline, stored overrides, and the resolved snapshot
/// every consumer reads.
pub(crate) struct RuntimeSettings {
    store: Option<SettingsStore>,
    baseline: SettingsSnapshot,
    /// What this process actually started with. Compared against the live
    /// snapshot to show which restart-class overrides aren't in force yet.
    booted: Arc<SettingsSnapshot>,
    current: ArcSwap<SettingsSnapshot>,
    overrides: RwLock<BTreeMap<SettingKey, StoredOverride>>,
}

impl RuntimeSettings {
    /// Connect to Postgres (if configured), load every override, and return the
    /// façade. Never fails: any problem is logged and leaves the façade serving
    /// the baseline, so an unreadable settings table cannot un-cap or un-throttle
    /// anything.
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

    /// A façade serving exactly `snapshot`, with no persistence behind it. The
    /// shape when there's no database, and the constructor tests use.
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

    /// The resolved settings. Cheap enough to call per event: one atomic load and
    /// a reference-count bump.
    pub(crate) fn snapshot(&self) -> Arc<SettingsSnapshot> {
        self.current.load_full()
    }

    /// What this process started with. A restart-class override shows up in
    /// [`Self::snapshot`] the moment it is recorded but doesn't reach the running
    /// subsystem until the next boot, and comparing the two is what lets the
    /// panel say so instead of leaving the operator to remember.
    pub(crate) fn booted(&self) -> Arc<SettingsSnapshot> {
        Arc::clone(&self.booted)
    }

    /// The deployed values an override layers on top of — environment where set,
    /// compiled default otherwise. Shown as the value clearing a setting returns
    /// it to.
    pub(crate) fn baseline(&self) -> &SettingsSnapshot {
        &self.baseline
    }

    /// Whether overrides can be recorded. `false` means the panel is read-only;
    /// the settings themselves keep working on the baseline.
    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    /// The stored overrides, for showing provenance beside each value.
    pub(crate) async fn overrides(&self) -> BTreeMap<SettingKey, StoredOverride> {
        self.overrides.read().await.clone()
    }

    /// Set an override, or clear it back to the deployed value with `raw = None`.
    ///
    /// Validated before it is written and written before it is served, so a value
    /// the panel accepted is one the database holds and the live snapshot never
    /// claims a setting that persistence didn't take. Returns the new snapshot,
    /// which the caller propagates to the consumers that can't pull (see
    /// `crate::discord::settings_changed`).
    ///
    /// # Errors
    ///
    /// Returns an error if persistence is unavailable, the value doesn't parse,
    /// the result would be internally inconsistent, or the write fails. The
    /// message is plain-language: it reaches the operator in the panel.
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
        // Held across the whole read-modify-write, so two concurrent changes
        // can't compose a snapshot out of each other's half-applied state.
        let mut overrides = self.overrides.write().await;
        let mut candidate = overrides.clone();
        match raw {
            Some(raw) => {
                // Parsed against the baseline first, so the message names this
                // value rather than some interaction with another override.
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
