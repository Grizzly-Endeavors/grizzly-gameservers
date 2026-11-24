use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use sqlx::AssertSqlSafe;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::DbConfig;

const HOME_POOL_MAX_CONNECTIONS: u32 = 4;
const GUILD_CONFIG_POOL_MAX_CONNECTIONS: u32 = 2;

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

const HOME_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS home_channels (\
        channel_id TEXT PRIMARY KEY, \
        guild_id TEXT, \
        added_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    ALTER TABLE home_channels ADD COLUMN IF NOT EXISTS guild_id TEXT";

struct HomeStore {
    pool: PgPool,
}

impl HomeStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, HOME_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(HOME_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply home_channels schema")?;
        Ok(Self { pool })
    }

    async fn load_all(&self) -> Result<Vec<(String, Option<String>)>> {
        sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT channel_id, guild_id FROM home_channels ORDER BY added_at",
        )
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

#[derive(Default)]
struct HomeCache {
    by_channel: HashMap<u64, Option<u64>>,
    order: Vec<u64>,
}

impl HomeCache {
    fn add(&mut self, channel: u64, guild: u64) {
        if self.by_channel.insert(channel, Some(guild)).is_none() {
            self.order.push(channel);
        }
    }

    fn remove(&mut self, channel: u64) {
        self.by_channel.remove(&channel);
        self.order.retain(|id| *id != channel);
    }

    fn home_for_guild(&self, guild: u64) -> Option<u64> {
        self.order
            .iter()
            .find(|channel| self.by_channel.get(*channel) == Some(&Some(guild)))
            .copied()
    }
}

pub(crate) struct HomeChannels {
    store: Option<HomeStore>,
    cache: RwLock<HomeCache>,
}

pub(crate) enum HomeToggle {
    Added,
    Removed,
    Unavailable,
}

impl HomeChannels {
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
            Ok(rows) => {
                let mut cache = HomeCache::default();
                for (channel, guild) in &rows {
                    let Ok(channel) = channel.parse::<u64>() else {
                        warn!(id = %channel, "skipping home channel with unparseable id");
                        continue;
                    };
                    let guild = guild.as_deref().and_then(|id| id.parse::<u64>().ok());
                    if cache.by_channel.insert(channel, guild).is_none() {
                        cache.order.push(channel);
                    }
                }
                info!(
                    home_channels = cache.order.len(),
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
            cache: RwLock::new(HomeCache::default()),
        }
    }

    pub(crate) async fn is_home(&self, channel: u64) -> bool {
        self.cache.read().await.by_channel.contains_key(&channel)
    }

    pub(crate) async fn home_for_guild(&self, guild: u64) -> Option<u64> {
        self.cache.read().await.home_for_guild(guild)
    }

    pub(crate) async fn toggle(&self, channel: u64, guild: u64) -> Result<HomeToggle> {
        let Some(store) = &self.store else {
            return Ok(HomeToggle::Unavailable);
        };
        let currently_home = self.cache.read().await.by_channel.contains_key(&channel);
        if currently_home {
            store.remove(channel).await?;
            self.cache.write().await.remove(channel);
            Ok(HomeToggle::Removed)
        } else {
            store.add(channel, guild).await?;
            self.cache.write().await.add(channel, guild);
            Ok(HomeToggle::Added)
        }
    }
}

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

#[derive(Clone, Copy)]
pub(crate) enum GrantTier {
    Admin,
    Manager,
}

#[derive(Clone, Copy)]
pub(crate) enum Principal {
    Role,
    User,
}

impl GrantTier {
    fn table(self, principal: Principal) -> &'static str {
        match (self, principal) {
            (Self::Admin, Principal::Role) => "guild_admin_roles",
            (Self::Admin, Principal::User) => "guild_admin_users",
            (Self::Manager, Principal::Role) => "guild_manager_roles",
            (Self::Manager, Principal::User) => "guild_manager_users",
        }
    }

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

#[derive(Clone, Debug, Default)]
pub(crate) struct GrantSet {
    pub(crate) roles: HashSet<u64>,
    pub(crate) users: HashSet<u64>,
}

#[derive(Clone, Debug, Default)]
struct GuildGrants {
    admins: GrantSet,
    managers: GrantSet,
}

pub(crate) enum ConfigChange {
    Added,
    Removed,
    Unchanged,
    Unavailable,
}

struct GuildConfigStore {
    pool: PgPool,
}

impl GuildConfigStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, GUILD_CONFIG_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(GUILD_CONFIG_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply guild_config schema")?;
        Ok(Self { pool })
    }

    async fn load_all(&self) -> Result<HashMap<u64, GuildGrants>> {
        let mut map: HashMap<u64, GuildGrants> = HashMap::new();
        for tier in [GrantTier::Admin, GrantTier::Manager] {
            for principal in [Principal::Role, Principal::User] {
                let table = tier.table(principal);
                let column = principal.column();
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

    async fn add(&self, tier: GrantTier, principal: Principal, guild: u64, id: u64) -> Result<()> {
        let table = tier.table(principal);
        let column = principal.column();
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

    async fn remove(
        &self,
        tier: GrantTier,
        principal: Principal,
        guild: u64,
        id: u64,
    ) -> Result<()> {
        let table = tier.table(principal);
        let column = principal.column();
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

pub(crate) struct GuildConfig {
    store: Option<GuildConfigStore>,
    cache: RwLock<HashMap<u64, GuildGrants>>,
}

impl GuildConfig {
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

    pub(crate) fn is_available(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) async fn admins(&self, guild: u64) -> GrantSet {
        self.grants(GrantTier::Admin, guild).await
    }

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

    pub(crate) async fn add_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Admin, Principal::Role, guild, role)
            .await
    }

    pub(crate) async fn remove_admin_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Admin, Principal::Role, guild, role)
            .await
    }

    pub(crate) async fn add_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Admin, Principal::User, guild, user)
            .await
    }

    pub(crate) async fn remove_admin_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Admin, Principal::User, guild, user)
            .await
    }

    pub(crate) async fn add_manager_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Manager, Principal::Role, guild, role)
            .await
    }

    pub(crate) async fn remove_manager_role(&self, guild: u64, role: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Manager, Principal::Role, guild, role)
            .await
    }

    pub(crate) async fn add_manager_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.add_grant(GrantTier::Manager, Principal::User, guild, user)
            .await
    }

    pub(crate) async fn remove_manager_user(&self, guild: u64, user: u64) -> Result<ConfigChange> {
        self.remove_grant(GrantTier::Manager, Principal::User, guild, user)
            .await
    }
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
