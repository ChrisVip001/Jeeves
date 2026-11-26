//! Durable initial task dispatch. Unknown claimed work is never submitted again.
use anyhow::{Result, ensure};
use sqlx::{FromRow, SqlitePool};

#[derive(Clone, Debug, FromRow)]
pub struct DispatchRow {
    pub task_id: String,
    pub session_id: String,
    pub turn_id: String,
    pub message: String,
    pub state: String,
    pub attachments_json: String,
}
/// A lease that stopped being renewed: the worker that held it is gone or stuck.
#[derive(Clone, Debug, FromRow)]
pub struct ExpiredLease {
    pub task_id: String,
    pub owner: String,
    pub turn_id: String,
}
#[derive(FromRow)]
struct ProgressRow {
    id: String,
    title: String,
    status: String,
    last_stop_reason: Option<String>,
    acceptance_json: String,
    waiting_for_dependencies: bool,
}
#[derive(Clone)]
pub struct TaskDispatch {
    pool: SqlitePool,
}
impl TaskDispatch {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    /// Bind a new task to evidence from the predecessor's current or a later run.
    pub async fn add_dependency_on(
        conn: &mut sqlx::SqliteConnection,
        task_id: &str,
        prerequisite_id: &str,
    ) -> Result<()> {
        let inserted = sqlx::query(
            "INSERT INTO task_dependencies(task_id,prerequisite_id,first_completion_id)
             SELECT ?,p.id,COALESCE((SELECT c.id + (p.status <> 'done')
                 FROM task_completions c WHERE c.task_id=p.id ORDER BY c.id DESC LIMIT 1),0)
             FROM agent_tasks p WHERE p.id=?",
        )
        .bind(task_id)
        .bind(prerequisite_id)
        .execute(conn)
        .await?
        .rows_affected();
        ensure!(
            inserted == 1,
            "Task prerequisite disappeared before creation"
        );
        Ok(())
    }

    pub async fn unresolved_dependencies(&self, task_id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT prerequisite_id FROM pending_task_dependencies WHERE task_id=? ORDER BY prerequisite_id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn has_ready_dependents(&self, prerequisite_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_dependencies r
                JOIN ready_task_dispatches d ON d.task_id=r.task_id
                WHERE r.prerequisite_id=?)",
        )
        .bind(prerequisite_id)
        .fetch_one(&self.pool)
        .await?)
    }
    pub async fn enqueue(&self, task_id: &str, message: &str) -> Result<DispatchRow> {
        self.enqueue_with_attachments(task_id, message, "[]").await
    }
    pub async fn enqueue_with_attachments(
        &self,
        task_id: &str,
        message: &str,
        attachments: &str,
    ) -> Result<DispatchRow> {
        self.enqueue_with_origin(task_id, message, attachments, None)
            .await
    }
    pub async fn enqueue_with_origin(
        &self,
        task_id: &str,
        message: &str,
        attachments: &str,
        origin_message_id: Option<&str>,
    ) -> Result<DispatchRow> {
        Self::enqueue_on(
            &mut *self.pool.acquire().await?,
            task_id,
            message,
            attachments,
            origin_message_id,
        )
        .await
    }

    pub async fn enqueue_on(
        conn: &mut sqlx::SqliteConnection,
        task_id: &str,
        message: &str,
        attachments: &str,
        origin_message_id: Option<&str>,
    ) -> Result<DispatchRow> {
        ensure!(
            !message.trim().is_empty() && message.len() <= 64_000,
            "task message must contain 1 to 64000 bytes"
        );
        let row=sqlx::query_as::<_,DispatchRow>("INSERT INTO task_dispatch(task_id,session_id,turn_id,message,attachments_json,origin_message_id)
            SELECT id, 'task-work:' || lower(hex(randomblob(16))), 'task-turn:' || lower(hex(randomblob(16))), ?, ?, ?
            FROM agent_tasks WHERE id=? AND status='pending' AND current_turn_id IS NULL AND NOT EXISTS(SELECT 1 FROM responsibility_predecessors WHERE task_id=agent_tasks.id)
            RETURNING task_id,session_id,turn_id,message,state,attachments_json")
            .bind(message).bind(attachments).bind(origin_message_id).bind(task_id).fetch_optional(conn).await?;
        row.ok_or_else(|| {
            anyhow::anyhow!("only a pending task without an execution can be started")
        })
    }
    pub async fn get(&self, task_id: &str) -> Result<Option<DispatchRow>> {
        Ok(sqlx::query_as(
            "SELECT task_id,session_id,turn_id,message,state,attachments_json FROM task_dispatch WHERE task_id=?",
        )
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?)
    }
    pub async fn claim(&self) -> Result<Option<DispatchRow>> {
        Ok(sqlx::query_as(
            "UPDATE task_dispatch SET state='claimed' WHERE task_id=(
            SELECT task_id FROM ready_task_dispatches ORDER BY dispatch_order LIMIT 1)
            AND NOT EXISTS(SELECT 1 FROM task_dispatch WHERE state='claimed')
            RETURNING task_id,session_id,turn_id,message,state,attachments_json",
        )
        .fetch_optional(&self.pool)
        .await?)
    }
    /// Claim the oldest pending dispatch while fewer than `max_workers` are claimed, and lease it
    /// to `owner` for `ttl_secs`. One transaction: the claim and its lease appear together.
    pub async fn claim_leased(
        &self,
        owner: &str,
        ttl_secs: i64,
        max_workers: i64,
    ) -> Result<Option<DispatchRow>> {
        let mut tx = self.pool.begin().await?;
        let row: Option<DispatchRow> = sqlx::query_as(
            "UPDATE task_dispatch SET state='claimed' WHERE task_id=(
            SELECT task_id FROM ready_task_dispatches ORDER BY dispatch_order LIMIT 1)
            AND (SELECT COUNT(*) FROM task_dispatch WHERE state='claimed')<?
            RETURNING task_id,session_id,turn_id,message,state,attachments_json",
        )
        .bind(max_workers.max(1))
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = &row {
            sqlx::query("INSERT OR REPLACE INTO task_leases(task_id,owner,expires_at) VALUES(?,?,CAST(strftime('%s','now') AS INTEGER)+?)")
                .bind(&row.task_id).bind(owner).bind(ttl_secs).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(row)
    }
    /// Renew a lease. `false` when it is gone (swept) or belongs to someone else.
    pub async fn heartbeat(&self, task_id: &str, owner: &str, ttl_secs: i64) -> Result<bool> {
        Ok(sqlx::query("UPDATE task_leases SET expires_at=CAST(strftime('%s','now') AS INTEGER)+? WHERE task_id=? AND owner=?")
            .bind(ttl_secs).bind(task_id).bind(owner).execute(&self.pool).await?.rows_affected()==1)
    }
    /// Take every expired lease away exactly once. Its dispatch becomes `interrupted`, so it is
    /// never claimed again; the caller decides whether a checkpoint resumes or a human reviews.
    pub async fn expire_stale(&self) -> Result<Vec<ExpiredLease>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<ExpiredLease> = sqlx::query_as(
            "SELECT l.task_id,l.owner,d.turn_id FROM task_leases l JOIN task_dispatch d ON d.task_id=l.task_id
             WHERE d.state='claimed' AND l.expires_at<CAST(strftime('%s','now') AS INTEGER)",
        )
        .fetch_all(&mut *tx)
        .await?;
        for row in &rows {
            sqlx::query(
                "UPDATE task_dispatch SET state='interrupted' WHERE task_id=? AND state='claimed'",
            )
            .bind(&row.task_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM task_leases WHERE expires_at<CAST(strftime('%s','now') AS INTEGER) OR task_id NOT IN (SELECT task_id FROM task_dispatch WHERE state='claimed')")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(rows)
    }
    /// Reopens a finished conversational task for a follow-up run. `acceptance_json` is the
    /// follow-up's own checks (`[]` when it named none); the previous run's checks never carry over.
    pub async fn continue_finished(
        &self,
        task_id: &str,
        message: &str,
        attachments: &str,
        message_id: &str,
        acceptance_json: &str,
    ) -> Result<()> {
        ensure!(
            !message.trim().is_empty() && message.len() <= 64_000,
            "Invalid task follow-up"
        );
        let mut tx = self.pool.begin().await?;
        let changed = sqlx::query(
            "UPDATE agent_tasks SET status='pending',finished_at=NULL,current_turn_id=NULL,
                last_stop_reason=NULL,acceptance_json=?,checks_json='[]',rounds_used=0
             WHERE id=? AND status='done' AND goal_condition IS NULL AND EXISTS(
                 SELECT 1 FROM task_dispatch d WHERE d.task_id=agent_tasks.id AND d.state='finished'
                 AND (agent_tasks.acceptance_json='[]' OR d.origin_message_id IS NOT NULL)
             )",
        )
        .bind(acceptance_json)
        .bind(task_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        ensure!(
            changed == 1,
            "Only a completed conversational task can accept a new follow-up; inspect other task states first"
        );
        sqlx::query("UPDATE task_dispatch SET message=?,attachments_json=?,turn_id='task-turn:'||lower(hex(randomblob(16))),state='pending' WHERE task_id=? AND state='finished'")
            .bind(message).bind(attachments).bind(task_id).execute(&mut *tx).await?;
        Self::record_amendment(&mut tx, task_id, message_id, message).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Requeue a settled, read-only research continuation explicitly requested by its owner.
    /// A generic needs-attention task must still use the reconciliation path.
    pub async fn continue_research(
        &self,
        task_id: &str,
        message: &str,
        message_id: &str,
        expected_reason: &str,
    ) -> Result<()> {
        ensure!(
            !message.trim().is_empty() && message.len() <= 64_000,
            "Invalid research follow-up"
        );
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE agent_tasks SET status='pending',finished_at=NULL,current_turn_id=NULL,
                last_stop_reason=NULL,checks_json='[]',rounds_used=0
             WHERE id=? AND status='needs_attention' AND last_stop_reason=?
             AND EXISTS(SELECT 1 FROM research_evidence WHERE task_id=agent_tasks.id)
             AND EXISTS(SELECT 1 FROM task_dispatch WHERE task_id=agent_tasks.id AND state='finished')"
        ).bind(task_id).bind(expected_reason).execute(&mut *tx).await?.rows_affected();
        ensure!(
            changed == 1,
            "Research is not waiting on a safe, settled continuation"
        );
        let changed = sqlx::query("UPDATE task_dispatch SET message=?,attachments_json='[]',turn_id='task-turn:'||lower(hex(randomblob(16))),state='pending' WHERE task_id=? AND state='finished'")
            .bind(message).bind(task_id).execute(&mut *tx).await?.rows_affected();
        ensure!(
            changed == 1,
            "Research dispatch changed before continuation"
        );
        Self::record_amendment(&mut tx, task_id, message_id, message).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn record_amendment(
        conn: &mut sqlx::SqliteConnection,
        task_id: &str,
        message_id: &str,
        instruction: &str,
    ) -> Result<()> {
        sqlx::query("INSERT INTO task_amendments(task_id,message_id,instruction) VALUES(?,?,?)")
            .bind(task_id)
            .bind(message_id)
            .bind(instruction)
            .execute(conn)
            .await?;
        Ok(())
    }
    pub async fn append_pending(
        &self,
        task_id: &str,
        message: &str,
        message_id: &str,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let changed=sqlx::query("UPDATE task_dispatch SET message=message||char(10)||char(10)||? WHERE task_id=? AND state='pending' AND length(message)+length(?)<=64000")
            .bind(message).bind(task_id).bind(message).execute(&mut *tx).await?.rows_affected()==1;
        if changed {
            Self::record_amendment(&mut tx, task_id, message_id, message).await?;
        }
        tx.commit().await?;
        Ok(changed)
    }
    /// Bounded, owner-scoped progress without exposing another task's working transcript.
    pub async fn progress(&self, owner_session: &str) -> Result<Vec<serde_json::Value>> {
        let rows: Vec<ProgressRow> = sqlx::query_as(
            "SELECT t.id,t.title,t.status,t.last_stop_reason,t.acceptance_json,
                    EXISTS(SELECT 1 FROM pending_task_dependencies w WHERE w.task_id=t.id) AS waiting_for_dependencies
             FROM agent_tasks t WHERE t.session_id=? AND (EXISTS(SELECT 1 FROM task_dispatch d WHERE d.task_id=t.id) OR EXISTS(SELECT 1 FROM responsibility_predecessors w WHERE w.task_id=t.id)) ORDER BY t.rowid DESC LIMIT 5")
            .bind(owner_session).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let status = if row.status == "pending" && row.waiting_for_dependencies {
                    "waiting_for_dependencies".to_string()
                } else if row.status == "done" && row.acceptance_json == "[]" {
                    "produced_unverified".to_string()
                } else {
                    row.status
                };
                serde_json::json!({
                    "id":row.id,
                    "title":row.title.chars().take(200).collect::<String>(),
                    "status":status,
                    "reason":row.last_stop_reason.map(|r|r.chars().take(500).collect::<String>())
                })
            })
            .collect())
    }

    /// A human pause before checkpoint admission may return the untouched dispatch to pending.
    pub async fn release_unstarted(&self, task_id: &str) -> Result<bool> {
        let changed=sqlx::query("UPDATE task_dispatch SET state='pending' WHERE task_id=? AND state='claimed'
            AND NOT EXISTS(SELECT 1 FROM turn_checkpoints c WHERE c.turn_id=task_dispatch.turn_id)
            AND EXISTS(SELECT 1 FROM agent_tasks t WHERE t.id=task_dispatch.task_id AND t.status IN ('pending','suspended') AND t.current_turn_id IS NULL)")
            .bind(task_id).execute(&self.pool).await?.rows_affected();
        if changed == 1 {
            self.drop_lease(task_id).await?;
        }
        Ok(changed == 1)
    }
    pub async fn resume_unstarted(&self, task_id: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let changed=sqlx::query("UPDATE agent_tasks SET status='pending',current_turn_id=NULL,suspend_reason=NULL,updated_at=datetime('now')
            WHERE id=? AND status IN ('suspended','needs_attention')
            AND EXISTS(SELECT 1 FROM task_dispatch d WHERE d.task_id=agent_tasks.id
                AND NOT EXISTS(SELECT 1 FROM turn_checkpoints c WHERE c.turn_id=d.turn_id)
                AND NOT EXISTS(SELECT 1 FROM task_runs r WHERE r.id=d.turn_id))")
            .bind(task_id).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("DELETE FROM task_pause_fences WHERE task_id=?")
                .bind(task_id)
                .execute(&mut *tx)
                .await?;

            sqlx::query("UPDATE task_dispatch SET state=CASE WHEN state IN ('finished','interrupted') THEN 'pending' ELSE state END,finished_at=NULL WHERE task_id=?")
                .bind(task_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(changed == 1)
    }
    pub async fn finish(&self, task_id: &str) -> Result<()> {
        sqlx::query("UPDATE task_dispatch SET state='finished',finished_at=datetime('now') WHERE task_id=? AND state='claimed'")
            .bind(task_id).execute(&self.pool).await?;
        self.drop_lease(task_id).await
    }
    async fn drop_lease(&self, task_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM task_leases WHERE task_id=?")
            .bind(task_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    /// Checkpoint recovery owns dispatched work. A claim without a checkpoint needs review.
    pub async fn reconcile(&self) -> Result<Vec<String>> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let missing: Vec<String> = sqlx::query_scalar(
            "SELECT task_id FROM task_dispatch d WHERE state='claimed'
            AND NOT EXISTS(SELECT 1 FROM turn_checkpoints c WHERE c.turn_id=d.turn_id)",
        )
        .fetch_all(&mut *tx)
        .await?;
        for id in &missing {
            sqlx::query("UPDATE agent_tasks SET status='needs_attention',suspend_reason='Task dispatch was interrupted before its checkpoint; review before retrying',updated_at=datetime('now') WHERE id=? AND status IN ('pending','running','suspended')")
                .bind(id).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE task_dispatch SET state='interrupted' WHERE state='claimed'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM task_leases")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(missing)
    }
}
