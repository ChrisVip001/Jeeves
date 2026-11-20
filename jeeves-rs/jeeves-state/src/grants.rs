//! Action grants and durable pending approvals (migration 0003).
//!
//! A grant is bound to account, resource, action and destination (and, when it carries one, the
//! exact content hash), so a changed target or changed content asks again.

use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Grant {
    pub id: String,
    pub account: String,
    pub resource: String,
    pub action: String,
    pub destination: String,
    pub content_hash: Option<String>,
    pub scope: String,
    pub expires_at: Option<i64>,
    pub revoked_version: i64,
    pub created_at: i64,
}

/// What is being asked right now.
#[derive(Debug, Clone)]
pub struct GrantRequest<'a> {
    pub account: &'a str,
    pub resource: &'a str,
    pub action: &'a str,
    pub destination: &'a str,
    pub content_hash: Option<&'a str>,
    pub now: i64,
}

/// Pure: does this grant cover this request? Everything must match exactly; a grant with a
/// content hash needs the same hash; expired or revoked grants never cover.
pub fn grant_covers(grant: &Grant, req: &GrantRequest<'_>) -> bool {
    if grant.revoked_version != 0 {
        return false;
    }
    if grant.expires_at.is_some_and(|at| at <= req.now) {
        return false;
    }
    if grant.account != req.account
        || grant.resource != req.resource
        || grant.action != req.action
        || grant.destination != req.destination
    {
        return false;
    }
    match grant.content_hash.as_deref() {
        Some(hash) => req.content_hash == Some(hash),
        None => true,
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct PendingRow {
    pub turn_id: String,
    pub request_id: String,
    pub session_id: String,
    pub tool: String,
    pub args_preview: String,
    pub reason: String,
    pub version: i64,
    pub expires_at: i64,
}

#[derive(Clone)]
pub struct Grants {
    pool: SqlitePool,
}

impl Grants {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn add(&self, g: &Grant) -> Result<()> {
        sqlx::query(
            "INSERT INTO action_grants (id, account, resource, action, destination, content_hash,
             scope, expires_at, revoked_version, created_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(&g.id)
        .bind(&g.account)
        .bind(&g.resource)
        .bind(&g.action)
        .bind(&g.destination)
        .bind(&g.content_hash)
        .bind(&g.scope)
        .bind(g.expires_at)
        .bind(g.revoked_version)
        .bind(g.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn revoke(&self, id: &str) -> Result<()> {
        sqlx::query("UPDATE action_grants SET revoked_version = revoked_version + 1 WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The first grant covering `req`; a `once` grant is spent by this call.
    pub async fn consume(&self, req: &GrantRequest<'_>) -> Result<Option<Grant>> {
        let rows = sqlx::query_as::<_, Grant>(
            "SELECT * FROM action_grants WHERE account = ? AND resource = ? AND action = ?
             ORDER BY created_at",
        )
        .bind(req.account)
        .bind(req.resource)
        .bind(req.action)
        .fetch_all(&self.pool)
        .await?;
        let Some(found) = rows.into_iter().find(|g| grant_covers(g, req)) else {
            return Ok(None);
        };
        if found.scope == "once" {
            let spent = sqlx::query(
                "UPDATE action_grants SET revoked_version = revoked_version + 1 WHERE id = ? AND revoked_version = 0",
            )
            .bind(&found.id)
            .execute(&self.pool)
            .await?.rows_affected();
            if spent == 0 {
                return Ok(None);
            }
        }
        Ok(Some(found))
    }

    pub async fn pending_put(&self, p: &PendingRow) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO pending_approvals
             (turn_id, request_id, session_id, tool, args_preview, reason, version, expires_at)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(&p.turn_id)
        .bind(&p.request_id)
        .bind(&p.session_id)
        .bind(&p.tool)
        .bind(&p.args_preview)
        .bind(&p.reason)
        .bind(p.version)
        .bind(p.expires_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pending_remove(&self, turn_id: &str, request_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM pending_approvals WHERE turn_id = ? AND request_id = ?")
            .bind(turn_id)
            .bind(request_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn pending_all(&self) -> Result<Vec<PendingRow>> {
        Ok(
            sqlx::query_as::<_, PendingRow>("SELECT * FROM pending_approvals ORDER BY expires_at")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// Restart: drop what already expired, bump the version of the rest (so an answer to the
    /// pre-restart question is refused), return the survivors.
    pub async fn pending_restore(&self, now: i64) -> Result<Vec<PendingRow>> {
        sqlx::query("DELETE FROM pending_approvals WHERE expires_at <= ?")
            .bind(now)
            .execute(&self.pool)
            .await?;
        sqlx::query("UPDATE pending_approvals SET version = version + 1")
            .execute(&self.pool)
            .await?;
        self.pending_all().await
    }
}

#[cfg(test)]
#[path = "grants_tests.rs"]
mod grants_tests;
