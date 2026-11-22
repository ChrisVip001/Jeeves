//! A scheduled wake may be attempted once. Keep the proposal and the result across restarts.
use sqlx::{FromRow, SqlitePool};

#[derive(FromRow)]
pub struct Work {
    pub wake_key: String,
    pub plan_json: Option<String>,
    pub event_json: Option<String>,
    pub due_ms: Option<i64>,
    pub responsibility_id: String,
    pub sources_json: String,
    pub digest_json: Option<String>,
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
        "SELECT wake_key,coalesce(outcome,status) AS status,body,detail,tools_used,wall_ms FROM responsibility_runs WHERE responsibility_id=? ORDER BY rowid DESC LIMIT 50",
    )
    .bind(id)
    .fetch_all(pool)
    .await
}

pub async fn take(pool: &SqlitePool, now_ms: i64) -> sqlx::Result<Option<Work>> {
    take_key(pool, now_ms, None).await
}

pub async fn take_key(
    pool: &SqlitePool,
    now_ms: i64,
    key: Option<&str>,
) -> sqlx::Result<Option<Work>> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let candidate: Option<Work> = sqlx::query_as(
        "SELECT w.source_key AS wake_key, r.id AS responsibility_id, r.sources_json,
                r.digest_json,
                r.plan_json,w.event_json,coalesce(w.due_ms,CASE WHEN r.digest_json IS NOT NULL
                    THEN CAST(substr(w.source_key,length(w.target_task)+2) AS INTEGER)*60000 END) AS due_ms,
                r.budget_json,r.expires_ms,w.created_at
         FROM wake_inbox w JOIN responsibilities r ON w.target_task='responsibility:' || r.id
         WHERE w.state='pending' AND (r.plan_json IS NOT NULL OR r.research_task_id IS NOT NULL OR r.digest_json IS NOT NULL)
           AND w.responsibility_revision=r.revision AND r.status='active' AND r.expires_ms>? AND (? IS NULL OR w.source_key=?)
           AND NOT EXISTS (SELECT 1 FROM responsibility_runs p WHERE p.wake_key=w.source_key)
           AND NOT EXISTS (SELECT 1 FROM responsibility_runs p WHERE p.responsibility_id=r.id AND p.status IN ('preparing','awaiting','sending'))
         ORDER BY w.rowid LIMIT 1",
    )
    .bind(now_ms)
    .bind(key).bind(key)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(work) = &candidate {
        sqlx::query(
            "INSERT INTO responsibility_runs(wake_key,responsibility_id,status,lease_until_ms,progress_ms)
             VALUES(?,?,'preparing',?,?)",
        ).bind(&work.wake_key).bind(&work.responsibility_id)
            .bind(now_ms + LEASE_MS).bind(now_ms)
            .execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(candidate)
}

const LEASE_MS: i64 = 30_000;

pub async fn active(pool: &SqlitePool, key: &str, now: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM responsibility_runs p JOIN wake_inbox w ON w.source_key=p.wake_key
        JOIN responsibilities r ON r.id=p.responsibility_id
        WHERE p.wake_key=? AND p.status='preparing' AND p.lease_until_ms>? AND w.state='pending'
          AND r.status='active' AND r.revision=w.responsibility_revision AND r.expires_ms>?)")
        .bind(key).bind(now).bind(now).fetch_one(pool).await
}

pub async fn heartbeat(pool: &SqlitePool, key: &str, now_ms: i64) -> sqlx::Result<bool> {
    let updated = sqlx::query(
        "UPDATE responsibility_runs SET lease_until_ms=?
         WHERE wake_key=? AND status IN ('preparing','awaiting','sending') AND lease_until_ms>?
         AND (status!='preparing' OR progress_ms>?)
         AND EXISTS(SELECT 1 FROM wake_inbox w JOIN responsibilities r ON r.id=responsibility_id
                    WHERE w.source_key=wake_key AND w.state='pending' AND r.status='active'
                    AND r.revision=w.responsibility_revision AND r.expires_ms>?)",
    )
    .bind(now_ms + LEASE_MS)
    .bind(key)
    .bind(now_ms)
    .bind(now_ms - 90_000)
    .bind(now_ms)
    .execute(pool)
    .await?;
    Ok(updated.rows_affected() == 1)
}

pub async fn record_tools(pool: &SqlitePool, key: &str, tools: i32) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE responsibility_runs SET tools_used=? WHERE wake_key=? AND status='preparing'",
    )
    .bind(tools)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn proposal(pool: &SqlitePool, key: &str, body: &str, tools: i32) -> sqlx::Result<()> {
    sqlx::query("UPDATE responsibility_runs SET body=?,tools_used=?,status='awaiting',progress_ms=unixepoch('subsec')*1000 WHERE wake_key=? AND status='preparing'")
        .bind(body).bind(tools).bind(key).execute(pool).await?;
    Ok(())
}

pub async fn outcome(
    pool: &SqlitePool,
    key: &str,
    status: &str,
    detail: &str,
    body: Option<&str>,
    tools: i32,
) -> sqlx::Result<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("UPDATE responsibility_runs SET body=?,tools_used=?,outcome=? WHERE wake_key=? AND status='preparing'")
        .bind(body).bind(tools).bind(status).bind(key).execute(&mut *tx).await?;
    finish_on(&mut tx, key, "draft", detail, 0).await?;
    tx.commit().await
}

pub struct Occurrence<'a> {
    pub id: &'a str,
    pub key: &'a str,
    pub revision: i32,
    pub due_ms: Option<i64>,
    pub data: Option<&'a str>,
    pub skip: Option<&'a str>,
}

pub async fn admit_on(
    conn: &mut sqlx::SqliteConnection,
    wake: Occurrence<'_>,
) -> sqlx::Result<bool> {
    let state = if wake.skip.is_some() {
        "delivered"
    } else {
        "pending"
    };
    let inserted = sqlx::query(
        "INSERT INTO wake_inbox(source_key,target_task,responsibility_revision,due_ms,event_json,state)
         VALUES(?,'responsibility:'||?,?,?,?,?) ON CONFLICT(source_key) DO NOTHING",
    ).bind(wake.key).bind(wake.id).bind(wake.revision).bind(wake.due_ms)
     .bind(wake.data).bind(state).execute(&mut *conn).await?.rows_affected() == 1;
    if let Some(reason) = wake.skip.filter(|_| inserted) {
        sqlx::query(
            "INSERT INTO responsibility_runs(wake_key,responsibility_id,status,outcome,detail)
                     VALUES(?,?,'draft','skipped',?)",
        )
        .bind(wake.key)
        .bind(wake.id)
        .bind(reason)
        .execute(conn)
        .await?;
    }
    Ok(inserted)
}

/// The last local boundary before POST. Pausing and resuming holds the original wake permanently.
pub async fn admit_send(pool: &SqlitePool, key: &str, now_ms: i64) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE responsibility_runs SET status='sending',tools_used=tools_used+1
         WHERE wake_key=? AND status='awaiting' AND (lease_until_ms IS NULL OR lease_until_ms>unixepoch('subsec')*1000)
         AND EXISTS (SELECT 1 FROM wake_inbox WHERE source_key=wake_key AND state='pending')
         AND EXISTS (SELECT 1 FROM responsibilities WHERE id=responsibility_id AND revision=(SELECT responsibility_revision FROM wake_inbox WHERE source_key=wake_key) AND status='active' AND expires_ms>?)")
        .bind(key).bind(now_ms).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn current_send(pool: &SqlitePool, key: &str, now: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM responsibility_runs p JOIN wake_inbox w ON w.source_key=p.wake_key JOIN responsibilities r ON r.id=p.responsibility_id WHERE p.wake_key=? AND p.status='sending' AND (p.lease_until_ms IS NULL OR p.lease_until_ms>?) AND w.state='pending' AND r.status='active' AND r.revision=w.responsibility_revision AND r.expires_ms>?)").bind(key).bind(now).bind(now).fetch_one(pool).await
}

pub async fn finish(
    pool: &SqlitePool,
    key: &str,
    status: &str,
    detail: &str,
    wall: i32,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    finish_on(&mut tx, key, status, detail, wall).await?;
    tx.commit().await
}

pub async fn finish_on(
    conn: &mut sqlx::SqliteConnection,
    key: &str,
    status: &str,
    detail: &str,
    wall: i32,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE responsibility_runs SET status=?,detail=?,wall_ms=? WHERE wake_key=? AND status NOT IN ('confirmed','draft','denied','review')")
        .bind(status)
        .bind(detail)
        .bind(wall)
        .bind(key)
        .execute(&mut *conn)
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
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn deliver_chat(
    pool: &SqlitePool,
    key: &str,
    body: &str,
    reads: i32,
    now: i64,
) -> sqlx::Result<bool> {
    let mut tx = pool.begin().await?;
    let claimed=sqlx::query("UPDATE responsibility_runs SET status='confirmed',body=?,tools_used=?,detail='Delivered in main chat'
        WHERE wake_key=? AND status='preparing' AND EXISTS(SELECT 1 FROM wake_inbox w JOIN responsibilities r
        ON r.id=responsibility_runs.responsibility_id WHERE w.source_key=responsibility_runs.wake_key AND w.state='pending'
        AND (lease_until_ms IS NULL OR lease_until_ms>?) AND r.revision=w.responsibility_revision AND r.status='active' AND r.expires_ms>?)")
        .bind(body).bind(reads).bind(key).bind(now).bind(now).execute(&mut *tx).await?.rows_affected();
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
    recover_before(pool, None).await
}

pub async fn recover_before(pool: &SqlitePool, cutoff: Option<i64>) -> sqlx::Result<Vec<String>> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let keys: Vec<String> = sqlx::query_scalar(
        "UPDATE responsibility_runs SET status='review',detail='Interrupted attempt; verify before any further delivery'
         WHERE status IN ('preparing','awaiting','sending') AND (? IS NULL OR lease_until_ms<=?) RETURNING wake_key")
        .bind(cutoff).bind(cutoff).fetch_all(&mut *tx).await?;
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
