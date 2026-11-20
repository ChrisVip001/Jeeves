//! Human-selected session policy. Missing rows preserve legacy runtime behavior.
use anyhow::{ensure, Result};
use sqlx::{FromRow, SqlitePool};

#[derive(Clone, FromRow)]
pub struct SessionPermissionRow {
    pub session_id: String,
    pub mode: String,
    pub revision: i32,
}

pub async fn load(pool: &SqlitePool) -> Result<Vec<SessionPermissionRow>> {
    Ok(
        sqlx::query_as("SELECT session_id, mode, revision FROM session_permissions")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn save(pool: &SqlitePool, session_id: &str, mode: &str, expected: i32) -> Result<i32> {
    ensure!(
        matches!(mode, "readOnly" | "standard" | "fullAccess"),
        "Invalid permission mode"
    );
    let revision = expected
        .checked_add(1)
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow::anyhow!("Permission revision exhausted"))?;
    let changed = if expected == 0 {
        sqlx::query("INSERT INTO session_permissions(session_id,mode,revision) SELECT id,?,1 FROM sessions WHERE id=? ON CONFLICT(session_id) DO NOTHING")
            .bind(mode).bind(session_id).execute(pool).await?.rows_affected()
    } else {
        sqlx::query(
            "UPDATE session_permissions SET mode=?,revision=? WHERE session_id=? AND revision=?",
        )
        .bind(mode)
        .bind(revision)
        .bind(session_id)
        .bind(expected)
        .execute(pool)
        .await?
        .rows_affected()
    };
    ensure!(
        changed == 1,
        "Session missing or permission revision changed; reload the session policy"
    );
    Ok(revision)
}
