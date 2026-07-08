//! The bot's durable state on foundation Postgres: the registry of no-mention
//! "home" channels where Gary answers without being `@`-mentioned, and the
//! per-guild admin config (admin roles and users) set at runtime via `/config`.
//!
//! [`HomeChannels`] and [`GuildConfig`] are the façades the rest of the bot
//! uses. Each keeps its state in memory (loaded once at startup, updated on each
//! mutation) so the per-message hot paths never touch the database, and each
//! **degrades gracefully**: if Postgres is unconfigured or unreachable at
//! startup, the bot still runs — mentions and slash commands work — and only the
//! DB-backed features (no-mention home channels, DB-configured guild admins) go
//! dark until a restart reconnects. Auth degrades **fail-closed**: with
//! `GuildConfig` unavailable, only the implicit admins (operators, guild owner)
//! are recognized; nobody new is admitted.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::DbConfig;

/// Postgres pool sizes for the bot's two small registries. Both are low-traffic —
/// loaded once at startup, then the occasional write on `/gary-home` or `/config`
/// — and the in-memory caches (not the pool) serve the per-message hot path, so
/// these are deliberately modest.
const HOME_POOL_MAX_CONNECTIONS: u32 = 4;
const GUILD_CONFIG_POOL_MAX_CONNECTIONS: u32 = 2;

/// Build a connection pool to the bot's foundation-Postgres database. Discord
/// snowflakes are stored as text throughout — they are unsigned 64-bit, which
/// `BIGINT` can't hold the top bit of, and text matches how the id is used
/// elsewhere.
async fn connect_pool(config: &DbConfig, max_connections: u32) -> Result<PgPool> {
    let options = PgConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .database(&config.database)
        .username(&config.user)
        .password(&config.password);
    PgPoolOptions::new()
        .max_connections(max_connections)
        .connect_with(options)
        .await
        .with_context(|| {
            format!(
                "failed to connect to postgres at {}:{}/{}",
                config.host, config.port, config.database
            )
        })
}

/// Schema for the home-channel registry. `guild_id` records which guild a home
/// channel belongs to (nullable for rows written before the guild-tenancy model)
/// so a guild's home channels can be listed; the per-message `is_home` check only
/// needs the channel id.
///
/// The `ADD COLUMN IF NOT EXISTS` is a migration: `home_channels` predates the
/// guild-tenancy model, so a database that ran the bot before `guild_id` was
/// added still has the old two-column table. `CREATE TABLE IF NOT EXISTS` never
/// alters an existing table, so without the explicit `ALTER` the `add` insert
/// (which binds `guild_id`) fails at runtime on those deployments.
const HOME_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS home_channels (\
        channel_id TEXT PRIMARY KEY, \
        guild_id TEXT, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    ALTER TABLE home_channels ADD COLUMN IF NOT EXISTS guild_id TEXT";

/// A connection pool for the home-channel registry, schema applied.
struct HomeStore {
    pool: PgPool,
}

impl HomeStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, HOME_POOL_MAX_CONNECTIONS).await?;
        // raw_sql runs the create-plus-migrate batch as one call.
        sqlx::raw_sql(HOME_SCHEMA)
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

    async fn add(&self, channel: u64, guild: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO home_channels (channel_id, guild_id) VALUES ($1, $2) \
             ON CONFLICT (channel_id) DO UPDATE SET guild_id = EXCLUDED.guild_id",
        )
        .bind(channel.to_string())
        .bind(guild.to_string())
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
    store: Option<HomeStore>,
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
        let store = match HomeStore::connect(config).await {
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
    /// cache. `guild` is recorded alongside so a guild's home channels can be
    /// listed. Returns [`HomeToggle::Unavailable`] (changing nothing) when
    /// persistence is down.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails; the cache is left
    /// untouched in that case so it never drifts from what was persisted.
    pub(crate) async fn toggle(&self, channel: u64, guild: u64) -> Result<HomeToggle> {
        let Some(store) = &self.store else {
            return Ok(HomeToggle::Unavailable);
        };
        let currently_home = self.cache.read().await.contains(&channel);
        if currently_home {
            store.remove(channel).await?;
            self.cache.write().await.remove(&channel);
            Ok(HomeToggle::Removed)
        } else {
            store.add(channel, guild).await?;
            self.cache.write().await.insert(channel);
            Ok(HomeToggle::Added)
        }
    }
}

/// Schema for the per-guild admin config. Two tables — one row per (guild, role)
/// and per (guild, user) — so add/remove is a plain insert/delete without array
/// columns. These sit *alongside* the operator seed (env `GAMESERVERS_ADMIN_USER_IDS`)
/// and the guild owner, all of which are admins regardless of these tables.
const GUILD_CONFIG_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS guild_admin_roles (\
        guild_id TEXT NOT NULL, \
        role_id TEXT NOT NULL, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        PRIMARY KEY (guild_id, role_id)); \
    CREATE TABLE IF NOT EXISTS guild_admin_users (\
        guild_id TEXT NOT NULL, \
        user_id TEXT NOT NULL, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        PRIMARY KEY (guild_id, user_id))";

/// The admin roles and users configured for one guild. Empty when the guild has
/// no DB config yet (or persistence is down) — callers still admit the implicit
/// admins (operators, guild owner) on top of this.
#[derive(Clone, Debug, Default)]
pub(crate) struct GuildAdmins {
    pub(crate) roles: HashSet<u64>,
    pub(crate) users: HashSet<u64>,
}

/// What a `/config` admin mutation did, for the command to report back.
pub(crate) enum ConfigChange {
    /// The role/user is now an admin (it wasn't before).
    Added,
    /// The role/user is no longer an admin (it was before).
    Removed,
    /// No change — it was already in the requested state.
    Unchanged,
    /// Persistence is down, so nothing changed.
    Unavailable,
}

/// A connection pool for the per-guild admin config, schema applied.
struct GuildConfigStore {
    pool: PgPool,
}

impl GuildConfigStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, GUILD_CONFIG_POOL_MAX_CONNECTIONS).await?;
        // raw_sql runs the multi-statement table batch as one call.
        sqlx::raw_sql(GUILD_CONFIG_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply guild_config schema")?;
        Ok(Self { pool })
    }

    /// Load every guild's admin roles and users into one map.
    async fn load_all(&self) -> Result<HashMap<u64, GuildAdmins>> {
        let mut map: HashMap<u64, GuildAdmins> = HashMap::new();
        let roles: Vec<(String, String)> =
            sqlx::query_as("SELECT guild_id, role_id FROM guild_admin_roles")
                .fetch_all(&self.pool)
                .await
                .context("failed to load guild admin roles")?;
        for (guild, role) in roles {
            if let (Ok(guild), Ok(role)) = (guild.parse(), role.parse()) {
                map.entry(guild).or_default().roles.insert(role);
            }
        }
        let users: Vec<(String, String)> =
            sqlx::query_as("SELECT guild_id, user_id FROM guild_admin_users")
                .fetch_all(&self.pool)
                .await
                .context("failed to load guild admin users")?;
        for (guild, user) in users {
            if let (Ok(guild), Ok(user)) = (guild.parse(), user.parse()) {
                map.entry(guild).or_default().users.insert(user);
            }
        }
        Ok(map)
    }

    async fn add_role(&self, guild: u64, role: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO guild_admin_roles (guild_id, role_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(guild.to_string())
        .bind(role.to_string())
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to add admin role {role} to guild {guild}"))?;
        Ok(())
    }

    async fn remove_role(&self, guild: u64, role: u64) -> Result<()> {
        sqlx::query("DELETE FROM guild_admin_roles WHERE guild_id = $1 AND role_id = $2")
            .bind(guild.to_string())
            .bind(role.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to remove admin role {role} from guild {guild}"))?;
        Ok(())
    }

    async fn add_user(&self, guild: u64, user: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO guild_admin_users (guild_id, user_id) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(guild.to_string())
        .bind(user.to_string())
        .execute(&self.pool)
        .await
        .with_context(|| format!("failed to add admin user {user} to guild {guild}"))?;
        Ok(())
    }

    async fn remove_user(&self, guild: u64, user: u64) -> Result<()> {
        sqlx::query("DELETE FROM guild_admin_users WHERE guild_id = $1 AND user_id = $2")
            .bind(guild.to_string())
            .bind(user.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to remove admin user {user} from guild {guild}"))?;
        Ok(())
    }
}

/// Per-guild admin config (admin roles + users), backed by Postgres and cached
/// in memory so the per-command auth check never touches the database. When
/// persistence is unavailable the façade reports empty config for every guild
/// and refuses mutations — auth then falls back fail-closed to the implicit
/// admins only (see module docs).
pub(crate) struct GuildConfig {
    store: Option<GuildConfigStore>,
    cache: RwLock<HashMap<u64, GuildAdmins>>,
}

impl GuildConfig {
    /// Connect to Postgres (if configured), load all guild config, and return the
    /// façade. Never fails: any problem is logged and leaves the façade in its
    /// disabled state so the rest of the bot keeps working.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            return Self::disabled();
        };
        let store = match GuildConfigStore::connect(config).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; per-guild admin config disabled");
                return Self::disabled();
            }
        };
        match store.load_all().await {
            Ok(cache) => {
                info!(
                    guilds_configured = cache.len(),
                    "loaded per-guild admin config"
                );
                Self {
                    store: Some(store),
                    cache: RwLock::new(cache),
                }
            }
            Err(err) => {
                error!(error = ?err, "failed to load guild admin config; feature disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            store: None,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Whether this façade is backed by a live database. `false` means every
    /// lookup returns empty config and every mutation is [`ConfigChange::Unavailable`].
    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    /// The admin roles and users configured for `guild` (empty when none, or when
    /// persistence is down). Cloned out so the auth check holds no lock.
    pub(crate) async fn admins(&self, guild: u64) -> GuildAdmins {
        self.cache
            .read()
            .await
            .get(&guild)
            .cloned()
            .unwrap_or_default()
    }

    /// Add an admin role for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails; the cache is left
    /// untouched in that case so it never drifts from what was persisted.
    pub(crate) async fn add_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|a| a.roles.contains(&role))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.add_role(guild, role).await?;
        self.cache
            .write()
            .await
            .entry(guild)
            .or_default()
            .roles
            .insert(role);
        Ok(ConfigChange::Added)
    }

    /// Remove an admin role from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if !self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|a| a.roles.contains(&role))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.remove_role(guild, role).await?;
        if let Some(admins) = self.cache.write().await.get_mut(&guild) {
            admins.roles.remove(&role);
        }
        Ok(ConfigChange::Removed)
    }

    /// Add an admin user for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn add_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|a| a.users.contains(&user))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.add_user(guild, user).await?;
        self.cache
            .write()
            .await
            .entry(guild)
            .or_default()
            .users
            .insert(user);
        Ok(ConfigChange::Added)
    }

    /// Remove an admin user from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if !self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|a| a.users.contains(&user))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.remove_user(guild, user).await?;
        if let Some(admins) = self.cache.write().await.get_mut(&guild) {
            admins.users.remove(&user);
        }
        Ok(ConfigChange::Removed)
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
