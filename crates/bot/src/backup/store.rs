use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tracing::{error, info, warn};

use crate::config::DbConfig;
use crate::domain::{GameId, GuildId, InstanceName};

const ARCHIVE_INDEX_MAX_CONNECTIONS: u32 = 2;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArchiveRecord {
    pub(crate) guild: GuildId,
    pub(crate) name: InstanceName,
    pub(crate) game: GameId,
    pub(crate) tarball_key: String,
    pub(crate) manifest_key: String,
    pub(crate) size_bytes: i64,
    pub(crate) created_by: String,
    pub(crate) created_at: String,
}

const LATEST_QUERY: &str = "SELECT guild_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 AND name = $2 ORDER BY created_at DESC LIMIT 1";

const LIST_QUERY: &str = "SELECT DISTINCT ON (name) guild_id, name, game, tarball_key, \
    manifest_key, size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 ORDER BY name, created_at DESC";

const LIST_ALL_QUERY: &str = "SELECT DISTINCT ON (guild_id, name) guild_id, name, game, \
    tarball_key, manifest_key, size_bytes, created_by, created_at::text FROM archives \
    ORDER BY guild_id, name, created_at DESC";

const ROWS_FOR_NAME: &str = "SELECT guild_id, name, game, tarball_key, manifest_key, \
    size_bytes, created_by, created_at::text FROM archives \
    WHERE guild_id = $1 AND name = $2 ORDER BY created_at DESC";

const DELETE_NAME: &str = "DELETE FROM archives WHERE guild_id = $1 AND name = $2";

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

pub(crate) struct ArchiveStore {
    pool: Option<PgPool>,
}

impl ArchiveStore {
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

    pub(crate) fn enabled(&self) -> bool {
        self.pool.is_some()
    }

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

    pub(crate) async fn list_latest_per_name(&self, guild: &str) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(LIST_QUERY)
            .bind(guild)
            .fetch_all(pool)
            .await
            .context("failed to list archives")?;
        Ok(rows.into_iter().map(row).collect())
    }

    pub(crate) async fn list_all_latest_per_name(&self) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(LIST_ALL_QUERY)
            .fetch_all(pool)
            .await
            .context("failed to list all archives")?;
        Ok(rows.into_iter().map(row).collect())
    }

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

    pub(crate) async fn rows_to_prune(&self, max_age_days: i32) -> Result<Vec<ArchiveRecord>> {
        let pool = self.pool()?;
        let rows = sqlx::query_as::<_, ArchiveRow>(ROWS_TO_PRUNE)
            .bind(max_age_days)
            .fetch_all(pool)
            .await
            .context("failed to list archives to prune")?;
        Ok(rows.into_iter().map(row).collect())
    }

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
