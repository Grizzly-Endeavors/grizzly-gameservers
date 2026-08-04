//! The archive index on foundation Postgres: the queryable catalog of torn-down
//! servers that can be recovered. It is a *rebuildable projection* of the S3
//! manifest sidecars (the durable source of truth), not the record of last
//! resort — so it earns its place only as the fast "what archives does this
//! guild have?" lookup that a bucket prefix scan can't answer once the instance
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
use crate::domain::{GameId, GuildId, InstanceName};

/// Postgres pool size for the archive index. Two is ample: the index is a
/// low-traffic catalog touched only on archive/recover/list, not a hot path.
const ARCHIVE_INDEX_MAX_CONNECTIONS: u32 = 2;

/// Idempotent schema for the archive catalog, applied at startup against the
/// bot's own database. Guild ids are text (Discord snowflakes overflow the sign
/// bit of `BIGINT`), matching `home_channels`.
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS archives (\
    id BIGSERIAL PRIMARY KEY, \
    guild_id TEXT NOT NULL, \
    name TEXT NOT NULL, \
    game TEXT NOT NULL, \
    tarball_key TEXT NOT NULL, \
    manifest_key TEXT NOT NULL, \
    size_bytes BIGINT NOT NULL, \
    created_by TEXT NOT NULL, \
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS archives_guild_name_idx ON archives (guild_id, name)";

/// One archived server: enough to recreate it and to show it in `/archives`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveRecord {
    /// Owning Discord guild id — the tenant the archive belongs to and the guild
    /// a recovered server is stamped back into.
    pub(crate) guild: GuildId,
    /// Server name, used both to recover and as the archive key segment.
    pub(crate) name: InstanceName,
    pub(crate) game: GameId,
    pub(crate) tarball_key: String,
    pub(crate) manifest_key: String,
    pub(crate) size_bytes: i64,
    pub(crate) created_by: String,
    /// RFC-3339-ish timestamp text (cast in SQL to avoid a chrono/time sqlx
    /// feature), for display only.
    pub(crate) created_at: String,
}

/// The most recent archive of a name in a guild.
const LATEST_QUERY: &str = "SELECT guild_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 AND name = $2 ORDER BY created_at DESC LIMIT 1";

/// The latest archive of each distinct name in a guild.
const LIST_QUERY: &str = "SELECT DISTINCT ON (name) guild_id, name, game, tarball_key, \
    manifest_key, size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 ORDER BY name, created_at DESC";

/// The latest archive of each distinct (guild, name) across every guild, for a
/// cross-guild operator's listing. Distinct on `(guild_id, name)` so same-named
/// archives in different guilds each survive rather than collapsing.
const LIST_ALL_QUERY: &str = "SELECT DISTINCT ON (guild_id, name) guild_id, name, game, \
    tarball_key, manifest_key, size_bytes, created_by, created_at::text FROM archives \
    ORDER BY guild_id, name, created_at DESC";

/// Every archive row of one name in a guild, newest first. Deletion works on the
/// whole name rather than a single row: a friend asks to "delete the smp
/// archive", and leaving older generations behind would mean the listing still
/// shows an entry after they were told it was gone.
const ROWS_FOR_NAME: &str = "SELECT guild_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 AND name = $2 ORDER BY created_at DESC";

const DELETE_NAME: &str = "DELETE FROM archives WHERE guild_id = $1 AND name = $2";

/// Archives past their retention window, excluding the newest of each
/// `(guild, name)`. The `NOT IN` subquery is what keeps the newest generation
/// regardless of age: an old archive is still the only copy of that world, and
/// pruning it on age alone would be silent data loss, not housekeeping.
const ROWS_TO_PRUNE: &str = "SELECT guild_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE created_at < now() - make_interval(days => $1) \
    AND id NOT IN (SELECT DISTINCT ON (guild_id, name) id FROM archives \
    ORDER BY guild_id, name, created_at DESC)";

const DELETE_BY_KEY: &str = "DELETE FROM archives WHERE tarball_key = $1";

type ArchiveRow = (String, String, String, String, String, i64, String, String);

fn row(columns: ArchiveRow) -> ArchiveRecord {
    let (guild, name, game, tarball_key, manifest_key, size_bytes, created_by, created_at) =
        columns;
    ArchiveRecord {
        guild: GuildId::new(guild),
        name: InstanceName::new(name),
        game: GameId::new(game),
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
            .max_connections(ARCHIVE_INDEX_MAX_CONNECTIONS)
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
             (guild_id, name, game, tarball_key, manifest_key, size_bytes, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(record.guild.as_str())
        .bind(record.name.as_str())
        .bind(record.game.as_str())
        .bind(&record.tarball_key)
        .bind(&record.manifest_key)
        .bind(record.size_bytes)
        .bind(&record.created_by)
        .execute(pool)
        .await
        .with_context(|| format!("failed to record archive {}", record.name))?;
        Ok(())
    }

    /// The most recent archive of `name` in `guild`, or `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn latest(&self, guild: &str, name: &str) -> Result<Option<ArchiveRecord>> {
        let pool = self.pool()?;
        let found = sqlx::query_as::<_, ArchiveRow>(LATEST_QUERY)
            .bind(guild)
            .bind(name)
            .fetch_optional(pool)
            .await
            .with_context(|| format!("failed to look up archive {name}"))?;
        Ok(found.map(row))
    }

    /// The latest archive of each distinct name in `guild`, for `/archives`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn list_latest_per_name(&self, guild: &str) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(LIST_QUERY)
            .bind(guild)
            .fetch_all(pool)
            .await
            .context("failed to list archives")?;
        Ok(rows.into_iter().map(row).collect())
    }

    /// The latest archive of each distinct (guild, name) across every guild, for a
    /// cross-guild operator's `/archives` and `/recover`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn list_all_latest_per_name(&self) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(LIST_ALL_QUERY)
            .fetch_all(pool)
            .await
            .context("failed to list all archives")?;
        Ok(rows.into_iter().map(row).collect())
    }

    /// Every archive generation recorded under `name` in `guild`, newest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn rows_for_name(
        &self,
        guild: &str,
        name: &str,
    ) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(ROWS_FOR_NAME)
            .bind(guild)
            .bind(name)
            .fetch_all(pool)
            .await
            .with_context(|| format!("failed to list archive generations for {name}"))?;
        Ok(rows.into_iter().map(row).collect())
    }

    /// Forget every archive generation of `name` in `guild`, returning how many
    /// rows went. The caller deletes the S3 objects; this only drops the index
    /// entries, and is called *after* the objects so a failure part-way leaves
    /// rows pointing at absent objects rather than objects nothing points at —
    /// the former is visible in the listing and fixable, the latter is a silent
    /// storage leak.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the delete fails.
    pub(crate) async fn delete_name(&self, guild: &str, name: &str) -> Result<u64> {
        let pool = self.pool()?;
        let done = sqlx::query(DELETE_NAME)
            .bind(guild)
            .bind(name)
            .execute(pool)
            .await
            .with_context(|| format!("failed to delete archives for {name}"))?;
        Ok(done.rows_affected())
    }

    /// Archives older than `max_age_days`, never including the newest generation
    /// of any `(guild, name)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the query fails.
    pub(crate) async fn rows_to_prune(&self, max_age_days: i32) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(ROWS_TO_PRUNE)
            .bind(max_age_days)
            .fetch_all(pool)
            .await
            .context("failed to list archives to prune")?;
        Ok(rows.into_iter().map(row).collect())
    }

    /// Forget one archive generation, identified by its tarball key.
    ///
    /// # Errors
    ///
    /// Returns an error if the store is disabled or the delete fails.
    pub(crate) async fn delete_by_key(&self, tarball_key: &str) -> Result<()> {
        let pool = self.pool()?;
        sqlx::query(DELETE_BY_KEY)
            .bind(tarball_key)
            .execute(pool)
            .await
            .with_context(|| format!("failed to delete archive row for {tarball_key}"))?;
        Ok(())
    }

    fn pool(&self) -> Result<&PgPool> {
        self.pool
            .as_ref()
            .context("the archive catalog isn't available (database not configured)")
    }
}
