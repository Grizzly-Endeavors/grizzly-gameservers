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

const GARY_MEMORY_POOL_MAX_CONNECTIONS: u32 = 2;

pub(crate) const GENERAL_SCOPE: &str = "general";

const MAX_RENDERED: usize = 40;

const MAX_NOTE_CHARS: usize = 500;

const GARY_MEMORY_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS gary_memories (\
        id BIGSERIAL PRIMARY KEY, \
        scope TEXT NOT NULL, \
        content TEXT NOT NULL, \
        created_by TEXT, \
        created_at TIMESTAMPTZ NOT NULL DEFAULT now()); \
    CREATE INDEX IF NOT EXISTS gary_memories_scope_idx ON gary_memories (scope)";

#[derive(Clone, Debug)]
pub(crate) struct Memory {
    pub(crate) id: i64,
    pub(crate) scope: String,
    pub(crate) content: String,
}

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

    async fn delete(&self, id: i64) -> Result<u64> {
        let result = sqlx::query("DELETE FROM gary_memories WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .with_context(|| format!("failed to forget memory {id}"))?;
        Ok(result.rows_affected())
    }
}

pub(crate) enum RememberOutcome {
    Saved(i64),
    TooLong { max: usize },
    Unavailable,
}

pub(crate) enum ForgetOutcome {
    Forgotten,
    NotFound,
    Unavailable,
}

pub(crate) struct GaryMemory {
    store: Option<MemoryStore>,
    cache: RwLock<Vec<Memory>>,
}

impl GaryMemory {
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

    pub(crate) async fn remember(
        &self,
        scope: &str,
        content: &str,
        created_by: Option<&str>,
    ) -> Result<RememberOutcome> {
        if content.chars().count() > MAX_NOTE_CHARS {
            return Ok(RememberOutcome::TooLong {
                max: MAX_NOTE_CHARS,
            });
        }
        let Some(store) = &self.store else {
            return Ok(RememberOutcome::Unavailable);
        };
        let content = crate::untrusted::scrub(content);
        let id = store.insert(scope, &content, created_by).await?;
        self.cache.write().await.push(Memory {
            id,
            scope: scope.to_owned(),
            content,
        });
        Ok(RememberOutcome::Saved(id))
    }

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

    pub(crate) async fn all(&self) -> Vec<Memory> {
        self.cache.read().await.clone()
    }

    pub(crate) async fn render_for_prompt(&self) -> String {
        render_memories(&self.cache.read().await)
    }
}

pub(crate) fn normalize_scope(raw: &str, game_ids: &[&str]) -> Option<String> {
    let normalized = raw.trim().to_lowercase();
    (normalized == GENERAL_SCOPE || game_ids.contains(&normalized.as_str())).then_some(normalized)
}

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
