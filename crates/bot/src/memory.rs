//! Gary's durable memory on foundation Postgres: the operational facts he learns
//! while running a game so he doesn't rediscover them every session (e.g.
//! "Palworld servers must be soft-stopped before a config edit applies").
//!
//! Facts are scoped to a catalog game id or the [`GENERAL_SCOPE`] bucket and are
//! **shared across every guild** — a quirk learned running one community's server
//! helps them all. They're self-authored by Gary at runtime through the
//! `remember`/`forget` tools and injected into his system prompt via
//! [`GaryMemory::render_for_prompt`].
//!
//! Like the other Postgres-backed façades ([`crate::store`]), [`GaryMemory`]
//! keeps its state in memory (loaded once at startup, updated on each mutation)
//! so the per-message prompt build never touches the database, and it **degrades
//! gracefully**: with Postgres unconfigured or unreachable, the bot still runs and
//! Gary simply has no durable memory until a restart reconnects.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::config::DbConfig;
use crate::store::connect_pool;

/// The memory pool is touched only at startup and on the occasional `remember`/
/// `forget`; the in-memory cache serves prompt builds, so a small pool suffices.
const GARY_MEMORY_POOL_MAX_CONNECTIONS: u32 = 2;

/// The scope for a fact that isn't tied to one game — general operational
/// knowledge that applies across the catalog.
pub(crate) const GENERAL_SCOPE: &str = "general";

/// Cap on facts rendered into the system prompt (newest first). Bounds prompt
/// growth if the store accumulates a lot of facts over time.
const MAX_RENDERED: usize = 40;

/// Schema for Gary's durable memory. `id` is a `BIGSERIAL` so `forget(id)` has a
/// stable handle; `scope` is a catalog game id or [`GENERAL_SCOPE`]; `created_by`
/// is the Discord user id as text (the snowflake-as-text convention used
/// throughout, since snowflakes overflow `BIGINT`).
const GARY_MEMORY_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS gary_memories (\
        id BIGSERIAL PRIMARY KEY, \
        scope TEXT NOT NULL, \
        content TEXT NOT NULL, \
        created_by TEXT, \
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS gary_memories_scope_idx ON gary_memories (scope)";

/// One durable fact Gary saved.
#[derive(Clone, Debug)]
pub(crate) struct Memory {
    pub(crate) id: i64,
    pub(crate) scope: String,
    pub(crate) content: String,
}

/// A connection pool for Gary's memory, schema applied.
struct MemoryStore {
    pool: PgPool,
}

impl MemoryStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, GARY_MEMORY_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(GARY_MEMORY_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply gary_memories schema")?;
        Ok(Self { pool })
    }

    /// Every saved fact, oldest first (ordered by id so the cache is stable).
    async fn load_all(&self) -> Result<Vec<Memory>> {
        let rows: Vec<(i64, String, String)> =
            sqlx::query_as("SELECT id, scope, content FROM gary_memories ORDER BY id")
                .fetch_all(&self.pool)
                .await
                .context("failed to load gary memories")?;
        Ok(rows
            .into_iter()
            .map(|(id, scope, content)| Memory { id, scope, content })
            .collect())
    }

    async fn insert(&self, scope: &str, content: &str, created_by: Option<&str>) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO gary_memories (scope, content, created_by) VALUES ($1, $2, $3) \
             RETURNING id",
        )
        .bind(scope)
        .bind(content)
        .bind(created_by)
        .fetch_one(&self.pool)
        .await
        .with_context(|| format!("failed to save a memory for scope {scope}"))
    }

    /// Delete the fact with `id`, returning how many rows were removed (0 if it
    /// was already gone).
    async fn delete(&self, id: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM gary_memories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to forget memory {id}"))?;
        Ok(result.rows_affected())
    }
}

/// What a `remember` did, for the tool to report back.
pub(crate) enum RememberOutcome {
    /// The fact was saved with this id.
    Saved(i64),
    /// Persistence is down, so nothing was saved.
    Unavailable,
}

/// What a `forget` did, for the tool or slash command to report back.
pub(crate) enum ForgetOutcome {
    /// The fact was deleted.
    Forgotten,
    /// No fact had that id.
    NotFound,
    /// Persistence is down, so nothing changed.
    Unavailable,
}

/// Gary's durable memory, backed by Postgres and cached in memory. When
/// persistence is unavailable the façade stays usable but holds no facts and
/// refuses writes (see module docs).
pub(crate) struct GaryMemory {
    store: Option<MemoryStore>,
    cache: RwLock<Vec<Memory>>,
}

impl GaryMemory {
    /// Connect to Postgres (if configured), load every saved fact, and return the
    /// façade. Never fails: any problem is logged and leaves memory disabled so
    /// the rest of the bot keeps working.
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            warn!(
                "DB_PASSWORD not set; Gary's durable memory disabled (he'll re-learn each session)"
            );
            return Self::disabled();
        };
        let store = match MemoryStore::connect(config).await {
            Ok(store) => store,
            Err(err) => {
                error!(error = ?err, "postgres unavailable; Gary's durable memory disabled");
                return Self::disabled();
            }
        };
        match store.load_all().await {
            Ok(memories) => {
                info!(memories = memories.len(), "loaded Gary's durable memory");
                Self {
                    store: Some(store),
                    cache: RwLock::new(memories),
                }
            }
            Err(err) => {
                error!(error = ?err, "failed to load Gary's memory; feature disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self {
            store: None,
            cache: RwLock::new(Vec::new()),
        }
    }

    /// Save a fact under `scope` (already normalized by the caller). Returns
    /// [`RememberOutcome::Unavailable`] (saving nothing) when persistence is down.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails; the cache is left
    /// untouched in that case so it never drifts from what was persisted.
    pub(crate) async fn remember(
        &self,
        scope: &str,
        content: &str,
        created_by: Option<&str>,
    ) -> Result<RememberOutcome> {
        let Some(store) = &self.store else {
            return Ok(RememberOutcome::Unavailable);
        };
        let id = store.insert(scope, content, created_by).await?;
        self.cache.write().await.push(Memory {
            id,
            scope: scope.to_owned(),
            content: content.to_owned(),
        });
        Ok(RememberOutcome::Saved(id))
    }

    /// Delete the fact with `id`. Returns [`ForgetOutcome::NotFound`] when no fact
    /// has that id, or [`ForgetOutcome::Unavailable`] when persistence is down.
    ///
    /// # Errors
    ///
    /// Returns an error only if the database write fails.
    pub(crate) async fn forget(&self, id: i64) -> Result<ForgetOutcome> {
        let Some(store) = &self.store else {
            return Ok(ForgetOutcome::Unavailable);
        };
        if store.delete(id).await? == 0 {
            return Ok(ForgetOutcome::NotFound);
        }
        self.cache.write().await.retain(|memory| memory.id != id);
        Ok(ForgetOutcome::Forgotten)
    }

    /// Every saved fact, for the admin review command. Cloned out so the caller
    /// holds no lock.
    pub(crate) async fn all(&self) -> Vec<Memory> {
        self.cache.read().await.clone()
    }

    /// The saved facts rendered for injection into Gary's system prompt (empty
    /// when there are none or persistence is down).
    pub(crate) async fn render_for_prompt(&self) -> String {
        render_memories(&self.cache.read().await)
    }
}

/// Normalize and validate a scope Gary supplied: trimmed and lowercased, and
/// either [`GENERAL_SCOPE`] or one of the known game ids. Returns `None` for an
/// unknown scope so the caller can hand Gary the valid list to retry with.
pub(crate) fn normalize_scope(raw: &str, game_ids: &[&str]) -> Option<String> {
    let normalized = raw.trim().to_lowercase();
    (normalized == GENERAL_SCOPE || game_ids.contains(&normalized.as_str())).then_some(normalized)
}

/// Render facts grouped by scope for the system prompt: newest facts first (up to
/// [`MAX_RENDERED`]), then grouped by scope in name order with each fact carrying
/// its id so Gary can `forget` it by number. Empty string when there are none.
fn render_memories(memories: &[Memory]) -> String {
    if memories.is_empty() {
        return String::new();
    }
    let mut recent: Vec<&Memory> = memories.iter().collect();
    recent.sort_by_key(|memory| std::cmp::Reverse(memory.id));
    recent.truncate(MAX_RENDERED);

    let mut by_scope: BTreeMap<&str, Vec<&Memory>> = BTreeMap::new();
    for memory in recent {
        by_scope
            .entry(memory.scope.as_str())
            .or_default()
            .push(memory);
    }

    by_scope
        .into_iter()
        .map(|(scope, mut facts)| {
            facts.sort_by_key(|memory| memory.id);
            let lines = facts
                .iter()
                .map(|memory| format!("  - #{}: {}", memory.id, memory.content))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{scope}:\n{lines}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "tests/memory.rs"]
mod tests;
