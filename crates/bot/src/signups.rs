use anyhow::{Context, Result};
use sqlx::postgres::PgPool;
use tracing::{error, warn};

use crate::config::DbConfig;
use crate::store::connect_pool;

const SIGNUPS_POOL_MAX_CONNECTIONS: u32 = 2;

/// Longest address accepted, per RFC 5321's path limit. Anything longer is a
/// paste accident or an attack, and storing it buys nothing.
pub(crate) const MAX_EMAIL_LEN: usize = 254;

pub(crate) const MAX_NOTE_LEN: usize = 2000;

const SIGNUPS_SCHEMA: &str = "\
    CREATE TABLE IF NOT EXISTS signups (\
        email TEXT PRIMARY KEY, \
        display_email TEXT NOT NULL, \
        wants_beta BOOLEAN NOT NULL DEFAULT false, \
        wants_updates BOOLEAN NOT NULL DEFAULT false, \
        note TEXT, \
        status TEXT, \
        created_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
        decided_at TIMESTAMPTZ, \
        decided_by TEXT, \
        emailed_at TIMESTAMPTZ, \
        email_error TEXT); \
    CREATE INDEX IF NOT EXISTS signups_pending_idx \
        ON signups (created_at) WHERE wants_beta AND status IS NULL; \
    CREATE INDEX IF NOT EXISTS signups_created_at_idx ON signups (created_at)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignupStatus {
    Approved,
    Denied,
}

impl SignupStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Denied => "denied",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SignupDecision {
    Recorded,
    AlreadyDecided,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SubmitOutcome {
    Recorded,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpdatesRow {
    pub(crate) email: String,
    pub(crate) first_asked_at: String,
    pub(crate) wants_beta: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SignupRow {
    pub(crate) email: String,
    pub(crate) display_email: String,
    pub(crate) wants_beta: bool,
    pub(crate) wants_updates: bool,
    pub(crate) note: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) created_at: String,
    pub(crate) decided_at: Option<String>,
    pub(crate) decided_by: Option<String>,
    pub(crate) emailed_at: Option<String>,
    pub(crate) email_error: Option<String>,
}

type SignupRowColumns = (
    String,
    String,
    bool,
    bool,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

impl From<SignupRowColumns> for SignupRow {
    fn from(columns: SignupRowColumns) -> Self {
        let (
            email,
            display_email,
            wants_beta,
            wants_updates,
            note,
            status,
            created_at,
            decided_at,
            decided_by,
            emailed_at,
            email_error,
        ) = columns;
        Self {
            email,
            display_email,
            wants_beta,
            wants_updates,
            note,
            status,
            created_at,
            decided_at,
            decided_by,
            emailed_at,
            email_error,
        }
    }
}

const PENDING_BETA: &str = "\
    SELECT email, display_email, wants_beta, wants_updates, note, status, \
           created_at::text, decided_at::text, decided_by, emailed_at::text, email_error \
    FROM signups WHERE wants_beta AND status IS NULL ORDER BY created_at LIMIT $1";

const RECENT_DECISIONS: &str = "\
    SELECT email, display_email, wants_beta, wants_updates, note, status, \
           created_at::text, decided_at::text, decided_by, emailed_at::text, email_error \
    FROM signups WHERE status IS NOT NULL ORDER BY decided_at DESC LIMIT $1";

const APPROVED_NOT_EMAILED: &str = "\
    SELECT email, display_email, wants_beta, wants_updates, note, status, \
           created_at::text, decided_at::text, decided_by, emailed_at::text, email_error \
    FROM signups WHERE status = 'approved' AND emailed_at IS NULL \
    ORDER BY decided_at DESC LIMIT $1";

const DECIDE_SIGNUP: &str = "\
    UPDATE signups SET status = $2, decided_at = now(), decided_by = $3 \
    WHERE email = $1 AND status IS NULL RETURNING email";

const MARK_EMAILED: &str =
    "UPDATE signups SET emailed_at = now(), email_error = NULL WHERE email = $1";

const MARK_EMAIL_FAILED: &str = "UPDATE signups SET email_error = $2 WHERE email = $1";

const UPDATES_EXPORT: &str = "\
    SELECT display_email, created_at::text, wants_beta FROM signups \
    WHERE wants_updates ORDER BY created_at LIMIT $1";

const UPDATES_COUNT: &str = "SELECT count(*) FROM signups WHERE wants_updates";

const SUBMISSIONS_LAST_24H: &str =
    "SELECT count(*) FROM signups WHERE created_at > now() - interval '24 hours'";

const UPSERT_SIGNUP: &str = "\
    INSERT INTO signups (email, display_email, wants_beta, wants_updates, note) \
    VALUES ($1, $2, $3, $4, $5) \
    ON CONFLICT (email) DO UPDATE SET \
        display_email = EXCLUDED.display_email, \
        wants_beta = signups.wants_beta OR EXCLUDED.wants_beta, \
        wants_updates = signups.wants_updates OR EXCLUDED.wants_updates, \
        note = COALESCE(EXCLUDED.note, signups.note)";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Submission {
    pub(crate) email: String,
    pub(crate) display_email: String,
    pub(crate) wants_beta: bool,
    pub(crate) wants_updates: bool,
    pub(crate) note: Option<String>,
}

pub(crate) fn normalize_email(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_EMAIL_LEN {
        return None;
    }
    if trimmed
        .chars()
        .any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    let (local, domain) = trimmed.split_once('@')?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') || !domain.contains('.') {
        return None;
    }
    Some(trimmed.to_lowercase())
}

pub(crate) fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

struct SignupStore {
    pool: PgPool,
}

impl SignupStore {
    async fn connect(config: &DbConfig) -> Result<Self> {
        let pool = connect_pool(config, SIGNUPS_POOL_MAX_CONNECTIONS).await?;
        sqlx::raw_sql(SIGNUPS_SCHEMA)
            .execute(&pool)
            .await
            .context("failed to apply signups schema")?;
        Ok(Self { pool })
    }

    async fn upsert(&self, submission: &Submission) -> Result<()> {
        sqlx::query(UPSERT_SIGNUP)
            .bind(&submission.email)
            .bind(&submission.display_email)
            .bind(submission.wants_beta)
            .bind(submission.wants_updates)
            .bind(submission.note.as_ref())
            .execute(&self.pool)
            .await
            .context("failed to record a signup")?;
        Ok(())
    }

    async fn rows(&self, sql: &'static str, limit: i64) -> Result<Vec<SignupRow>> {
        let rows: Vec<SignupRowColumns> = sqlx::query_as(sql)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("failed to read signups")?;
        Ok(rows.into_iter().map(SignupRow::from).collect())
    }

    async fn decide(&self, email: &str, status: SignupStatus, by: &str) -> Result<bool> {
        let row: Option<(String,)> = sqlx::query_as(DECIDE_SIGNUP)
            .bind(email)
            .bind(status.as_str())
            .bind(by)
            .fetch_optional(&self.pool)
            .await
            .context("failed to record a signup decision")?;
        Ok(row.is_some())
    }

    async fn set_email_result(&self, email: &str, error: Option<&str>) -> Result<()> {
        let sql = if error.is_some() {
            MARK_EMAIL_FAILED
        } else {
            MARK_EMAILED
        };
        let mut query = sqlx::query(sql).bind(email);
        if let Some(error) = error {
            query = query.bind(error);
        }
        query
            .execute(&self.pool)
            .await
            .context("failed to record the approval email's result")?;
        Ok(())
    }

    async fn updates_export(&self, limit: i64) -> Result<Vec<UpdatesRow>> {
        let rows: Vec<(String, String, bool)> = sqlx::query_as(UPDATES_EXPORT)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .context("failed to export the updates list")?;
        Ok(rows
            .into_iter()
            .map(|(email, first_asked_at, wants_beta)| UpdatesRow {
                email,
                first_asked_at,
                wants_beta,
            })
            .collect())
    }

    async fn count(&self, sql: &'static str) -> Result<i64> {
        sqlx::query_scalar::<_, i64>(sql)
            .fetch_one(&self.pool)
            .await
            .context("failed to count signups")
    }
}

pub(crate) struct Signups {
    store: Option<SignupStore>,
}

impl Signups {
    pub(crate) async fn connect(config: Option<&DbConfig>) -> Self {
        let Some(config) = config else {
            warn!("DB_PASSWORD not set; the beta signup queue is disabled (no signups recorded)");
            return Self::disabled();
        };
        match SignupStore::connect(config).await {
            Ok(store) => Self { store: Some(store) },
            Err(err) => {
                error!(error = ?err, "postgres unavailable; the beta signup queue is disabled");
                Self::disabled()
            }
        }
    }

    fn disabled() -> Self {
        Self { store: None }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.store.is_some()
    }

    pub(crate) async fn submit(&self, submission: &Submission) -> SubmitOutcome {
        let Some(store) = self.store.as_ref() else {
            warn!("a signup arrived with no database configured; it was not recorded");
            return SubmitOutcome::Unavailable;
        };
        match store.upsert(submission).await {
            Ok(()) => SubmitOutcome::Recorded,
            Err(err) => {
                error!(error = ?err, "failed to record a signup");
                SubmitOutcome::Unavailable
            }
        }
    }

    pub(crate) async fn pending_beta(&self, limit: i64) -> Vec<SignupRow> {
        self.read(PENDING_BETA, limit).await
    }

    pub(crate) async fn recent_decisions(&self, limit: i64) -> Vec<SignupRow> {
        self.read(RECENT_DECISIONS, limit).await
    }

    pub(crate) async fn approved_not_emailed(&self, limit: i64) -> Vec<SignupRow> {
        self.read(APPROVED_NOT_EMAILED, limit).await
    }

    pub(crate) async fn decide(
        &self,
        email: &str,
        status: SignupStatus,
        by: &str,
    ) -> SignupDecision {
        let Some(store) = self.store.as_ref() else {
            return SignupDecision::Unavailable;
        };
        match store.decide(email, status, by).await {
            Ok(true) => SignupDecision::Recorded,
            Ok(false) => SignupDecision::AlreadyDecided,
            Err(err) => {
                error!(error = ?err, "failed to record a signup decision");
                SignupDecision::Unavailable
            }
        }
    }

    pub(crate) async fn mark_emailed(&self, email: &str) {
        self.record_email_result(email, None).await;
    }

    pub(crate) async fn mark_email_failed(&self, email: &str, reason: &str) {
        self.record_email_result(email, Some(reason)).await;
    }

    async fn record_email_result(&self, email: &str, error: Option<&str>) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if let Err(err) = store.set_email_result(email, error).await {
            error!(error = ?err, "failed to record the approval email's result");
        }
    }

    pub(crate) async fn updates_export(&self, limit: i64) -> Vec<UpdatesRow> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        match store.updates_export(limit).await {
            Ok(rows) => rows,
            Err(err) => {
                error!(error = ?err, "failed to export the updates list");
                Vec::new()
            }
        }
    }

    pub(crate) async fn updates_count(&self) -> i64 {
        self.tally(UPDATES_COUNT).await
    }

    pub(crate) async fn submissions_last_24h(&self) -> i64 {
        self.tally(SUBMISSIONS_LAST_24H).await
    }

    async fn read(&self, sql: &'static str, limit: i64) -> Vec<SignupRow> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        match store.rows(sql, limit).await {
            Ok(rows) => rows,
            Err(err) => {
                error!(error = ?err, "failed to read the signup queue");
                Vec::new()
            }
        }
    }

    async fn tally(&self, sql: &'static str) -> i64 {
        let Some(store) = self.store.as_ref() else {
            return 0;
        };
        match store.count(sql).await {
            Ok(count) => count,
            Err(err) => {
                error!(error = ?err, "failed to count signups");
                0
            }
        }
    }
}

#[cfg(test)]
#[path = "tests/signups.rs"]
mod tests;
