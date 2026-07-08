//! The bot's durable state on foundation Postgres: the registry of no-mention
//! "home" channels where Gary answers without being `@`-mentioned.
//!
//! [`HomeChannels`] is the façade the rest of the bot uses. It keeps the home
//! set in memory (loaded once at startup, updated on each `/gary-home` toggle)
//! so the per-message "is this a home channel?" check never touches the
//! database, and it **degrades gracefully**: if Postgres is unconfigured or
//! unreachable at startup, the bot still runs — mentions and slash commands work
//! — and only the no-mention feature is disabled until a restart reconnects.

use std::collections::HashSet;

use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::DbConfig;

/// Schema for the bot's own database. Applied at startup against a database the
/// bot's role owns outright (per the foundation-stores model), so a plain
/// idempotent `CREATE TABLE IF NOT EXISTS` is the whole migration. Channel ids
/// are stored as text — Discord snowflakes are unsigned 64-bit, which `BIGINT`
/// can't hold the top bit of, and text matches how the id is used elsewhere.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS home_channels (\
    channel_id TEXT PRIMARY KEY, \
    added_at TIMESTAMPTZ NOT NULL DEFAULT now())";

/// A connection pool to the bot's foundation-Postgres database, with the schema
/// applied. Thin wrapper: all the bot needs today is the home-channel registry.
struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connect, apply the schema, and return the store.
    ///
    /// # Errors
    ///
    /// Returns an error if the pool can't be built or the schema can't be applied.
    async fn connect(config: &DbConfig) -> Result<Self> {
        let options = PgConnectOptions::new()
            .host(&config.host)
            .port(config.port)
            .database(&config.database)
            .username(&config.user)
            .password(&config.password);
        let pool = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to postgres at {}:{}/{}",
                    config.host, config.port, config.database
                )
            })?;
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply home_channels schema")?;
        Ok(Self { pool })
    }

    /// Every registered home channel id.
    async fn load_all(&self) -> Result<Vec<String>> {
        sqlx::query_scalar::<_, String>("SELECT channel_id FROM home_channels")
            .fetch_all(&self.pool)
            .await
            .context("failed to load home channels")
    }

    async fn add(&self, channel: u64) -> Result<()> {
        sqlx::query("INSERT INTO home_channels (channel_id) VALUES ($1) ON CONFLICT DO NOTHING")
            .bind(channel.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to add home channel {channel}"))?;
        Ok(())
    }

    async fn remove(&self, channel: u64) -> Result<()> {
        sqlx::query("DELETE FROM home_channels WHERE channel_id = $1")
            .bind(channel.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to remove home channel {channel}"))?;
        Ok(())
    }
}

/// The set of channels where Gary answers without a mention, backed by Postgres
/// and cached in memory. When persistence is unavailable the façade stays usable
/// but reports every channel as non-home and refuses toggles (see module docs).
pub(crate) struct HomeChannels {
    store: Option<PgStore>,
    cache: RwLock<HashSet<u64>>,
}

/// What a `/gary-home` toggle did, for the command to report back.
pub(crate) enum HomeToggle {
    /// The channel is now a home channel.
    Added,
    /// The channel is no longer a home channel.
    Removed,
    /// Persistence is down, so nothing changed.
    Unavailable,
}

impl HomeChannels {
    /// Connect to Postgres (if configured), load the home set, and return the
    /// façade. Never fails: any problem is logged and leaves persistence
    /// disabled so the rest of the bot keeps working.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            warn!("DB_PASSWORD not set; no-mention home channels disabled (mentions still work)");
            return Self::disabled();
        };
        let store = match PgStore::connect(config).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; no-mention home channels disabled");
                return Self::disabled();
            }
        };
        match store.load_all().await {
            Ok(ids) => {
                let cache: HashSet<u64> = ids.iter().filter_map(|id| id.parse().ok()).collect();
                info!(home_channels = cache.len(), "connected to postgres");
                Self {
                    store: Some(store),
                    cache: RwLock::new(cache),
                }
            }
            Err(err) => {
                error!(error = ?err, "failed to load home channels; feature disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            store: None,
            cache: RwLock::new(HashSet::new()),
        }
    }

    /// Whether `channel` is a registered home channel. Always `false` when
    /// persistence is disabled.
    pub(crate) async fn is_home(&self, channel: u64) -> bool {
        self.cache.read().await.contains(&channel)
    }

    /// Flip `channel`'s home status, persisting the change and updating the
    /// cache. Returns [`HomeToggle::Unavailable`] (changing nothing) when
    /// persistence is down.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails; the cache is left
    /// untouched in that case so it never drifts from what was persisted.
    pub(crate) async fn toggle(&self, channel: u64) -> Result<HomeToggle> {
        let Some(store) = &self.store else {
            return Ok(HomeToggle::Unavailable);
        };
        let currently_home = self.cache.read().await.contains(&channel);
        if currently_home {
            store.remove(channel).await?;
            self.cache.write().await.remove(&channel);
            Ok(HomeToggle::Removed)
        } else {
            store.add(channel).await?;
            self.cache.write().await.insert(channel);
            Ok(HomeToggle::Added)
        }
    }
}
