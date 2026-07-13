//! The bot's durable state on foundation Postgres: the registry of no-mention
//! "home" channels where Gary answers without being `@`-mentioned, and the
//! per-guild access config (admin and manager roles and users) set at runtime
//! via `/config`.
//!
//! [`HomeChannels`] and [`GuildConfig`] are the façades the rest of the bot
//! uses. Each keeps its state in memory (loaded once at startup, updated on each
//! mutation) so the per-message hot paths never touch the database, and each
//! **degrades gracefully**: if Postgres is unconfigured or unreachable at
//! startup, the bot still runs — mentions and slash commands work — and only the
//! DB-backed features (no-mention home channels, DB-configured guild admins and
//! managers) go dark until a restart reconnects. Auth degrades **fail-closed**:
//! with `GuildConfig` unavailable, only the implicit admins (operators, guild
//! owner) are recognized; nobody new — admin or manager — is admitted.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::AssertSqlSafe;
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
pub(crate) async fn connect_pool(config: &DbConfig, max_connections: u32) -> Result<PgPool> {
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
                let cache: HashSet<u64> = ids
                    .iter()
                    .filter_map(|id| {
                        if let Ok(parsed) = id.parse() {
                            Some(parsed)
                        } else {
                            warn!(id = %id, "skipping home channel with unparseable id");
                            None
                        }
                    })
                    .collect();
                info!(
                    home_channels = cache.len(),
                    "connected to postgres for no-mention home channels"
                );
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

/// Schema for the per-guild access config. Each tier (admin, manager) gets a
/// roles table and a users table — one row per (guild, role) and per (guild,
/// user) — so add/remove is a plain insert/delete without array columns. The
/// admin tables sit *alongside* the operator seed (env `GAMESERVERS_ADMIN_USER_IDS`)
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
        PRIMARY KEY (guild_id, user_id)); \
    CREATE TABLE IF NOT EXISTS guild_manager_roles (\
        guild_id TEXT NOT NULL, \
        role_id TEXT NOT NULL, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        PRIMARY KEY (guild_id, role_id)); \
    CREATE TABLE IF NOT EXISTS guild_manager_users (\
        guild_id TEXT NOT NULL, \
        user_id TEXT NOT NULL, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        PRIMARY KEY (guild_id, user_id))";

/// Which access tier a `/config` grant targets. The two tiers share identical
/// storage shape (a roles table + a users table), so the CRUD is generic over
/// this — only the table name differs.
#[derive(Clone, Copy)]
pub(crate) enum GrantTier {
    Admin,
    Manager,
}

/// Whether a grant names a Discord role or a user — selects the id column.
#[derive(Clone, Copy)]
pub(crate) enum Principal {
    Role,
    User,
}

impl GrantTier {
    /// The table holding this tier's grants for the given principal kind. All
    /// four are compile-time constants from closed enums — never user input, so
    /// interpolating them into SQL is safe.
    fn table(self, principal: Principal) -> &'static str {
        match (self, principal) {
            (Self::Admin, Principal::Role) => "guild_admin_roles",
            (Self::Admin, Principal::User) => "guild_admin_users",
            (Self::Manager, Principal::Role) => "guild_manager_roles",
            (Self::Manager, Principal::User) => "guild_manager_users",
        }
    }

    /// This tier's grant set within a guild's grants.
    fn set(self, grants: &GuildGrants) -> &GrantSet {
        match self {
            Self::Admin => &grants.admins,
            Self::Manager => &grants.managers,
        }
    }

    fn set_mut(self, grants: &mut GuildGrants) -> &mut GrantSet {
        match self {
            Self::Admin => &mut grants.admins,
            Self::Manager => &mut grants.managers,
        }
    }
}

impl Principal {
    fn column(self) -> &'static str {
        match self {
            Self::Role => "role_id",
            Self::User => "user_id",
        }
    }

    /// The role or user set within a grant set.
    fn ids(self, grants: &GrantSet) -> &HashSet<u64> {
        match self {
            Self::Role => &grants.roles,
            Self::User => &grants.users,
        }
    }

    fn ids_mut(self, grants: &mut GrantSet) -> &mut HashSet<u64> {
        match self {
            Self::Role => &mut grants.roles,
            Self::User => &mut grants.users,
        }
    }
}

/// The roles and users granted one tier for one guild. Empty when the guild has
/// no DB config yet (or persistence is down) — callers still admit the implicit
/// admins (operators, guild owner) on top of this.
#[derive(Clone, Debug, Default)]
pub(crate) struct GrantSet {
    pub(crate) roles: HashSet<u64>,
    pub(crate) users: HashSet<u64>,
}

/// Both access tiers' grants for one guild, as cached and loaded together.
#[derive(Clone, Debug, Default)]
struct GuildGrants {
    admins: GrantSet,
    managers: GrantSet,
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

    /// Load every guild's grants (both tiers, roles and users) into one map.
    async fn load_all(&self) -> Result<HashMap<u64, GuildGrants>> {
        let mut map: HashMap<u64, GuildGrants> = HashMap::new();
        for tier in [GrantTier::Admin, GrantTier::Manager] {
            for principal in [Principal::Role, Principal::User] {
                let table = tier.table(principal);
                let column = principal.column();
                // table/column come from closed enums (never user input), so the
                // interpolation is injection-safe — hence AssertSqlSafe.
                let select = format!("SELECT guild_id, {column} FROM {table}");
                let rows: Vec<(String, String)> = sqlx::query_as(AssertSqlSafe(select))
                    .fetch_all(&self.pool)
                    .await
                    .with_context(|| format!("failed to load {table}"))?;
                for (guild, id) in rows {
                    if let (Ok(guild), Ok(id)) = (guild.parse::<u64>(), id.parse::<u64>()) {
                        principal
                            .ids_mut(tier.set_mut(map.entry(guild).or_default()))
                            .insert(id);
                    } else {
                        warn!(
                            guild = %guild,
                            id = %id,
                            table,
                            "skipping admin/manager grant with unparseable id"
                        );
                    }
                }
            }
        }
        Ok(map)
    }

    /// Grant `id` (a role or user) the given tier in `guild`. Idempotent.
    async fn add(&self, tier: GrantTier, principal: Principal, guild: u64, id: u64) -> Result<()> {
        let table = tier.table(principal);
        let column = principal.column();
        // table/column come from closed enums (never user input) — injection-safe.
        let insert = format!(
            "INSERT INTO {table} (guild_id, {column}) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        );
        sqlx::query(AssertSqlSafe(insert))
            .bind(guild.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to add {id} to {table} for guild {guild}"))?;
        Ok(())
    }

    /// Revoke `id`'s grant of the given tier in `guild`. Idempotent.
    async fn remove(
        &self,
        tier: GrantTier,
        principal: Principal,
        guild: u64,
        id: u64,
    ) -> Result<()> {
        let table = tier.table(principal);
        let column = principal.column();
        // table/column come from closed enums (never user input) — injection-safe.
        let delete = format!("DELETE FROM {table} WHERE guild_id = $1 AND {column} = $2");
        sqlx::query(AssertSqlSafe(delete))
            .bind(guild.to_string())
            .bind(id.to_string())
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to remove {id} from {table} for guild {guild}"))?;
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
    cache: RwLock<HashMap<u64, GuildGrants>>,
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
    pub(crate) async fn admins(&self, guild: u64) -> GrantSet {
        self.grants(GrantTier::Admin, guild).await
    }

    /// The manager roles and users configured for `guild` (empty when none, or
    /// when persistence is down). Cloned out so the auth check holds no lock.
    pub(crate) async fn managers(&self, guild: u64) -> GrantSet {
        self.grants(GrantTier::Manager, guild).await
    }

    async fn grants(&self, tier: GrantTier, guild: u64) -> GrantSet {
        self.cache
            .read()
            .await
            .get(&guild)
            .map(|g| tier.set(g).clone())
            .unwrap_or_default()
    }

    /// Grant `id` (a role or user) the given tier in `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails; the cache is left
    /// untouched in that case so it never drifts from what was persisted.
    async fn add_grant(
        &self,
        tier: GrantTier,
        principal: Principal,
        guild: u64,
        id: u64,
    ) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|g| principal.ids(tier.set(g)).contains(&id))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.add(tier, principal, guild, id).await?;
        principal
            .ids_mut(tier.set_mut(self.cache.write().await.entry(guild).or_default()))
            .insert(id);
        Ok(ConfigChange::Added)
    }

    /// Revoke `id`'s grant of the given tier in `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    async fn remove_grant(
        &self,
        tier: GrantTier,
        principal: Principal,
        guild: u64,
        id: u64,
    ) -> Result<ConfigChange> {
        let Some(store) = &self.store else {
            return Ok(ConfigChange::Unavailable);
        };
        if !self
            .cache
            .read()
            .await
            .get(&guild)
            .is_some_and(|g| principal.ids(tier.set(g)).contains(&id))
        {
            return Ok(ConfigChange::Unchanged);
        }
        store.remove(tier, principal, guild, id).await?;
        if let Some(grants) = self.cache.write().await.get_mut(&guild) {
            principal.ids_mut(tier.set_mut(grants)).remove(&id);
        }
        Ok(ConfigChange::Removed)
    }

    /// Add an admin role for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn add_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Admin, Principal::Role, guild, role)
            .await
    }

    /// Remove an admin role from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Admin, Principal::Role, guild, role)
            .await
    }

    /// Add an admin user for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn add_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Admin, Principal::User, guild, user)
            .await
    }

    /// Remove an admin user from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Admin, Principal::User, guild, user)
            .await
    }

    /// Add a manager role for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn add_manager_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Manager, Principal::Role, guild, role)
            .await
    }

    /// Remove a manager role from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_manager_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Manager, Principal::Role, guild, role)
            .await
    }

    /// Add a manager user for `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn add_manager_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Manager, Principal::User, guild, user)
            .await
    }

    /// Remove a manager user from `guild`.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn remove_manager_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Manager, Principal::User, guild, user)
            .await
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
