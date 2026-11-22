//! A weekly wake may be attempted once. Keep the proposal and the result across restarts.
use sqlx::{FromRow, SqlitePool};

#[derive(FromRow)]
pub struct Work {
    pub wake_key: String,
    pub responsibility_id: String,
    pub sources_json: String,
    pub weekly_report_json: Option<String>,
    pub digest_json: Option<String>,
    pub slot: i64,
    pub budget_json: String,
    pub expires_ms: i64,
    pub created_at: String,
}

#[derive(FromRow)]
pub struct Report {
    pub wake_key: String,
    pub status: String,
    pub body: Option<String>,
    pub detail: String,
    pub tools_used: i32,
    pub wall_ms: i32,
}

pub async fn list(pool: &SqlitePool, id: &str) -> sqlx::Result<Vec<Report>> {
    sqlx::query_as(
        "SELECT * FROM weekly_reports WHERE responsibility_id=? ORDER BY rowid DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn take(pool: &SqlitePool, now_ms: i64) -> sqlx::Result<Option<Work>> {
    let candidate: Option<Work> = sqlx::query_as(
        "SELECT w.source_key AS wake_key, r.id AS responsibility_id, r.sources_json,
                r.weekly_report_json, r.digest_json, CAST(substr(w.source_key,length(w.target_task)+2) AS INTEGER) AS slot,r.budget_json, r.expires_ms, w.created_at
         FROM wake_inbox w JOIN responsibilities r ON w.target_task='responsibility:' || r.id
         WHERE w.state='pending' AND (r.weekly_report_json IS NOT NULL OR r.digest_json IS NOT NULL) AND w.responsibility_revision=r.revision
           AND r.status='active' AND r.expires_ms>?
           AND NOT EXISTS (SELECT 1 FROM weekly_reports p WHERE p.wake_key=w.source_key)
         ORDER BY w.rowid LIMIT 1",
    )
    .bind(now_ms)
    .fetch_optional(pool)
    .await?;
    let Some(work) = candidate else {
        return Ok(None);
    };
    let result = sqlx::query(
        "INSERT INTO weekly_reports(wake_key,responsibility_id,status,tools_used)
         SELECT source_key,?, 'preparing',1 FROM wake_inbox WHERE source_key=? AND state='pending'
         ON CONFLICT(wake_key) DO NOTHING",
    )
    .bind(&work.responsibility_id)
    .bind(&work.wake_key)
    .execute(pool)
    .await?;
    Ok((result.rows_affected() == 1).then_some(work))
}

pub async fn proposal(pool: &SqlitePool, key: &str, body: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE weekly_reports SET body=?,status='awaiting' WHERE wake_key=? AND status='preparing'")
        .bind(body).bind(key).execute(pool).await?;
    Ok(())
}

/// The last local boundary before POST. Pausing and resuming holds the original wake permanently.
pub async fn admit_send(pool: &SqlitePool, key: &str, now_ms: i64) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE weekly_reports SET status='sending',tools_used=tools_used+1
         WHERE wake_key=? AND status='awaiting'
         AND EXISTS (SELECT 1 FROM wake_inbox WHERE source_key=wake_key AND state='pending')
         AND EXISTS (SELECT 1 FROM responsibilities WHERE id=responsibility_id AND status='active' AND expires_ms>?)")
        .bind(key).bind(now_ms).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn finish(
    pool: &SqlitePool,
    key: &str,
    status: &str,
    detail: &str,
    wall: i32,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE weekly_reports SET status=?,detail=?,wall_ms=? WHERE wake_key=? AND status NOT IN ('confirmed','draft','denied','review')")
        .bind(status)
        .bind(detail)
        .bind(wall)
        .bind(key)
        .execute(&mut *tx)
        .await?;
    let wake_state = if matches!(status, "confirmed" | "draft") {
        "delivered"
    } else {
        "reconcile"
    };
    sqlx::query(
        "UPDATE wake_inbox SET state=?,settled_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
                 WHERE source_key=? AND state='pending'",
    )
    .bind(wake_state)
    .bind(key)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

pub async fn deliver_chat(
    pool: &SqlitePool,
    key: &str,
    body: &str,
    reads: i32,
    now: i64,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let claimed=sqlx::query("UPDATE weekly_reports SET status='confirmed',body=?,tools_used=?,detail='Delivered in main chat'
        WHERE wake_key=? AND status='preparing' AND EXISTS(SELECT 1 FROM wake_inbox w JOIN responsibilities r
        ON r.id=weekly_reports.responsibility_id WHERE w.source_key=weekly_reports.wake_key AND w.state='pending'
        AND r.revision=w.responsibility_revision AND r.status='active' AND r.expires_ms>?)")
        .bind(body).bind(reads).bind(key).bind(now).execute(&mut *tx).await?.rows_affected();
    if claimed == 1 {
        sqlx::query("INSERT INTO chat_log(role,content,session_id,source) VALUES('assistant',?,'jeeves-main',?)")
            .bind(body).bind(key).execute(&mut *tx).await?;
        sqlx::query("UPDATE wake_inbox SET state='delivered',settled_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE source_key=?")
            .bind(key).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(claimed == 1)
}

pub async fn recover(pool: &SqlitePool) -> sqlx::Result<Vec<String>> {
    let mut tx = pool.begin().await?;
    let keys: Vec<String> = sqlx::query_scalar(
        "UPDATE weekly_reports SET status='review',detail='Interrupted attempt; verify before any further delivery'
         WHERE status IN ('preparing','awaiting','sending') RETURNING wake_key")
        .fetch_all(&mut *tx).await?;
    for key in &keys {
        sqlx::query("UPDATE wake_inbox SET state='reconcile',settled_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
                     WHERE source_key=? AND state='pending'").bind(key).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM pending_approvals WHERE turn_id=?")
            .bind(key)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(keys)
}
