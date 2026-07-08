//! The archive index on foundation Postgres: the queryable catalog of torn-down
//! servers that can be recovered. It is a *rebuildable projection* of the S3
//! manifest sidecars (the durable source of truth), not the record of last
//! resort — so it earns its place only as the fast "what archives does this
//! channel have?" lookup that a bucket prefix scan can't answer once the instance
//! is gone.
//!
//! Automatic backups need no index (the live instance is their key), so only
//! archive/recover touch this. It **degrades gracefully** exactly like
//! [`crate::store::HomeChannels`]: with no database configured the store is
//! disabled and the archive/recover commands report that cleanly, while backups
//! and restore-from-backup keep working.

use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tracing::{error, info, warn};

use crate::config::DbConfig;

/// Idempotent schema for the archive catalog, applied at startup against the
/// bot's own database. Channel ids are text (Discord snowflakes overflow the sign
/// bit of `BIGINT`), matching `home_channels`.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS archives (\
    id BIGSERIAL PRIMARY KEY, \
    channel_id TEXT NOT NULL, \
    name TEXT NOT NULL, \
    game TEXT NOT NULL, \
    tarball_key TEXT NOT NULL, \
    manifest_key TEXT NOT NULL, \
    size_bytes BIGINT NOT NULL, \
    created_by TEXT NOT NULL, \
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS archives_channel_name_idx ON archives (channel_id, name)";

/// One archived server: enough to recreate it and to show it in `/archives`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveRecord {
    pub(crate) channel: String,
    /// Server name, used both to recover and as the archive key segment.
    pub(crate) name: String,
    pub(crate) game: String,
    pub(crate) tarball_key: String,
    pub(crate) manifest_key: String,
    pub(crate) size_bytes: i64,
    pub(crate) created_by: String,
    /// RFC-3339-ish timestamp text (cast in SQL to avoid a chrono/time sqlx
    /// feature), for display only.
    pub(crate) created_at: String,
}

/// The most recent archive of a name in a channel.
const LATEST_QUERY: &str = "SELECT channel_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE channel_id = $1 AND name = $2 ORDER BY created_at DESC LIMIT 1";

/// The latest archive of each distinct name in a channel.
const LIST_QUERY: &str = "SELECT DISTINCT ON (name) channel_id, name, game, tarball_key, \
    manifest_key, size_bytes, created_by, created_at::text FROM archives \
    WHERE channel_id = $1 ORDER BY name, created_at DESC";

type ArchiveRow = (String, String, String, String, String, i64, String, String);

fn row(columns: ArchiveRow) -> ArchiveRecord {
    let (channel, name, game, tarball_key, manifest_key, size_bytes, created_by, created_at) =
        columns;
    ArchiveRecord {
        channel,
        name,
        game,
        tarball_key,
        manifest_key,
        size_bytes,
        created_by,
        created_at,
    }
}

/// The archive catalog, or a disabled no-op when persistence isn't configured.
pub(crate) struct ArchiveStore {
    pool: Option<PgPool>,
}

impl ArchiveStore {
    /// Connect (if configured) and apply the schema. Never fails: any problem is
    /// logged and leaves the store disabled so the rest of the bot keeps working.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            warn!("DB_PASSWORD not set; server archive/recover disabled (backups still work)");
            return Self { pool: None };
        };
        match Self::open(config).await {
            Ok(pool) => {
                info!("connected to postgres for the archive catalog");
                Self { pool: Some(pool) }
            }
            Err(err) => {
                error!(error = ?err, "postgres unavailable; server archive/recover disabled");
                Self { pool: None }
            }
        }
    }

    async fn open(config: &DbConfig) -> Result<PgPool> {
        let options = PgConnectOptions::new()
            .host(&config.host)
            .port(config.port)
            .database(&config.database)
            .username(&config.user)
            .password(&config.password);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to postgres at {}:{}/{}",
                    config.host, config.port, config.database
                )
            })?;
        // `raw_sql` runs via the simple query protocol so the multi-statement
        // schema (table + index) executes as one batch; `query` would prepare it
        // and Postgres rejects multiple commands in a prepared statement.
        sqlx::raw_sql(SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply archives schema")?;
        Ok(pool)
    }

    /// Whether the catalog is backed by a live database.
    pub(crate) fn enabled(&self) -> bool {
        self.pool.is_some()
    }

    /// Record a new archive.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the insert fails.
    pub(crate) async fn insert(&self, record: &ArchiveRecord) -> Result<()> {
        let pool = self.pool()?;
        sqlx::query(
            "INSERT INTO archives \
             (channel_id, name, game, tarball_key, manifest_key, size_bytes, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&record.channel)
        .bind(&record.name)
        .bind(&record.game)
        .bind(&record.tarball_key)
        .bind(&record.manifest_key)
        .bind(record.size_bytes)
        .bind(&record.created_by)
        .execute(pool)
        .await
        .with_context(|| format!("failed to record archive {}", record.name))?;
        Ok(())
    }

    /// The most recent archive of `name` in `channel`, or `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn latest(&self, channel: &str, name: &str) -> Result<Option<ArchiveRecord>> {
        let pool = self.pool()?;
        let found = sqlx::query_as::<_, ArchiveRow>(LATEST_QUERY)
            .bind(channel)
            .bind(name)
            .fetch_optional(pool)
            .await
            .with_context(|| format!("failed to look up archive {name}"))?;
        Ok(found.map(row))
    }

    /// The latest archive of each distinct name in `channel`, for `/archives`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn list_latest_per_name(&self, channel: &str) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(LIST_QUERY)
            .bind(channel)
            .fetch_all(pool)
            .await
            .context("failed to list archives")?;
        Ok(rows.into_iter().map(row).collect())
    }

    fn pool(&self) -> Result<&PgPool> {
        self.pool
            .as_ref()
            .context("the archive catalog isn't available (database not configured)")
    }
}
