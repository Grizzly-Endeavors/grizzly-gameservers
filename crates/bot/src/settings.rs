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
//! the next boot rather than immediately.
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

use anyhow::{Context, Result, bail};
use arc_swap::ArcSwap;
use sqlx::postgres::PgPool;
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

/// One operator-tunable knob. The variants are the whole catalog: `as_str` is
/// what lands in the database, and every other method matches exhaustively, so
/// adding a variant without deciding its grammar is a compile error rather than a
/// silently half-wired setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SettingKey {
    GaryModel,
    GaryBaseUrl,
    GaryMaxRounds,
    ThrottleBurst,
    ThrottleRefillSeconds,
    ThrottleMaxConcurrent,
    GuildAllowlist,
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
        Self::ThrottleBurst,
        Self::ThrottleRefillSeconds,
        Self::ThrottleMaxConcurrent,
        Self::GuildAllowlist,
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
            Self::ThrottleBurst => "throttle.burst",
            Self::ThrottleRefillSeconds => "throttle.refill_seconds",
            Self::ThrottleMaxConcurrent => "throttle.max_concurrent",
            Self::GuildAllowlist => "guilds.allowlist",
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

    /// The grammar this key accepts, phrased to complete "needs …". Used in the
    /// rejection message, so it has to describe a value the operator could type.
    pub(crate) fn expected(self) -> &'static str {
        match self {
            Self::GaryModel => "a model tag like \"glm-5.2\"",
            Self::GaryBaseUrl => "a base URL like \"https://ollama.com/v1\"",
            Self::GaryMaxRounds
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
    pub(crate) throttle: ThrottlePolicy,
    /// Guilds admitted. Empty admits every guild — see
    /// [`BotConfig::guild_allowlist`](crate::config::BotConfig::guild_allowlist).
    pub(crate) guild_allowlist: Arc<[u64]>,
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
            throttle: config.throttle,
            guild_allowlist: config.guild_allowlist.clone().into(),
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

    /// Cross-field rules no single key can enforce alone.
    ///
    /// # Errors
    ///
    /// Returns a plain-language description of the inconsistency.
    fn coherence(&self) -> Result<()> {
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
            throttle: ThrottlePolicy::default(),
            guild_allowlist: Vec::new().into(),
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

/// The settings façade: the resolved snapshot every consumer reads.
pub(crate) struct RuntimeSettings {
    current: ArcSwap<SettingsSnapshot>,
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
                baseline
            }
        };
        info!(
            overrides = overrides.len(),
            "loaded runtime setting overrides"
        );
        Self {
            current: ArcSwap::new(Arc::new(resolved)),
        }
    }

    /// A façade serving exactly `snapshot`. The shape when there's no database,
    /// and the constructor tests and other façades use.
    pub(crate) fn fixed(snapshot: SettingsSnapshot) -> Self {
        Self {
            current: ArcSwap::new(Arc::new(snapshot)),
        }
    }

    /// The resolved settings. Cheap enough to call per event: one atomic load and
    /// a reference-count bump.
    pub(crate) fn snapshot(&self) -> Arc<SettingsSnapshot> {
        self.current.load_full()
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
