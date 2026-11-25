//! Shared control-plane leases for browser and desktop targets.
//!
//! A lease is the authoritative answer to "who is driving this target right now".
//! Execution backends keep their own generation for wire compatibility, but every
//! generation change must be mirrored here so emergency stop, handoff and audit
//! see one consistent state.

use anyhow::Result;
use sqlx::SqlitePool;

pub(crate) fn browser_lease_id(session: &str) -> String {
    format!("browser:{session}")
}

pub(crate) fn desktop_lease_id(task: &str) -> String {
    format!("desktop:{task}")
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn upsert(
    pool: &SqlitePool,
    lease_id: &str,
    task: &str,
    owner: Option<&str>,
    device: &str,
    epoch: i32,
    status: &str,
    reason: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO control_leases(lease_id,task_id,owner_session_id,device,epoch,status,reason,updated_at)
         VALUES(?,?,?,?,?,?,?,datetime('now'))
         ON CONFLICT(lease_id) DO UPDATE SET
           task_id=excluded.task_id,
           owner_session_id=excluded.owner_session_id,
           device=excluded.device,
           epoch=excluded.epoch,
           status=excluded.status,
           reason=excluded.reason,
           updated_at=datetime('now')
         WHERE control_leases.epoch < excluded.epoch
            OR control_leases.status <> excluded.status
            OR control_leases.reason IS NOT excluded.reason",
    )
    .bind(lease_id)
    .bind(task)
    .bind(owner)
    .bind(device)
    .bind(epoch)
    .bind(status)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn revoke_task(pool: &SqlitePool, task: &str, reason: &str) -> Result<()> {
    sqlx::query(
        "UPDATE control_leases SET epoch=epoch+1,status='revoked',reason=?,updated_at=datetime('now')
         WHERE task_id=? AND status<>'revoked'",
    )
    .bind(reason)
    .bind(task)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("leases.db");
        let pool = SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true),
        )
        .await
        .expect("pool");
        sqlx::query(
            "CREATE TABLE control_leases (
                lease_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, owner_session_id TEXT,
                device TEXT NOT NULL, epoch INTEGER NOT NULL, status TEXT NOT NULL,
                reason TEXT, expires_at TEXT, created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
        )
        .execute(&pool)
        .await
        .expect("schema");
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn lease_upsert_and_revoke_are_auditable() {
        let pool = pool().await;
        upsert(&pool, "desktop:t1", "t1", None, "desktop", 1, "agent", None)
            .await
            .expect("upsert");
        upsert(
            &pool,
            "desktop:t1",
            "t1",
            None,
            "desktop",
            1,
            "human",
            Some("takeover"),
        )
        .await
        .expect("re-upsert");
        let status: (String, Option<String>) =
            sqlx::query_as("SELECT status,reason FROM control_leases WHERE lease_id='desktop:t1'")
                .fetch_one(&pool)
                .await
                .expect("row");
        assert_eq!(status.0, "human");
        assert_eq!(status.1.as_deref(), Some("takeover"));
        revoke_task(&pool, "t1", "emergency stop")
            .await
            .expect("revoke");
        let revoked: (String, i32, Option<String>) = sqlx::query_as(
            "SELECT status,epoch,reason FROM control_leases WHERE lease_id='desktop:t1'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(revoked.0, "revoked");
        assert_eq!(revoked.1, 2);
        assert_eq!(revoked.2.as_deref(), Some("emergency stop"));
    }
}
