//! Durable production receipts inserted atomically by task status transitions.
use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow)]
pub struct CompletionRow {
    pub id: i64,
    pub task_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub is_goal: bool,
    pub status: String,
    pub reason: String,
    pub notify_target: Option<String>,
}
pub(crate) const RECEIPT_APPROVALS: &str = include_str!("receipt_approvals.sql");
pub(crate) const RECEIPT_STATUS: &str = include_str!("receipt_status.sql");

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptApproval {
    pub request_id: String,
    pub tool: String,
    pub outcome: String,
}

#[derive(Debug, FromRow)]
pub struct DeliveryRow {
    #[sqlx(json(nullable))]
    pub approval_decisions: Option<Vec<ReceiptApproval>>,
    pub status: String,
    pub summary: Option<String>,
    pub run_id: Option<String>,
    pub at: String,
}
#[derive(Clone)]
pub struct Completions {
    pool: SqlitePool,
}
impl Completions {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    pub fn schedules(&self) -> crate::schedule_completions::ScheduleCompletions {
        crate::schedule_completions::ScheduleCompletions::new(self.pool.clone())
    }
    pub async fn bind(&self, task_id: &str, requested: bool, target: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE agent_tasks SET completion_notify_requested=?,completion_notify_target=? WHERE id=?")
            .bind(requested).bind(target).bind(task_id).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn latest_delivery(&self, task_id: &str) -> Result<Option<DeliveryRow>> {
        Ok(sqlx::query_as(&format!("SELECT {RECEIPT_APPROVALS} AS approval_decisions, {RECEIPT_STATUS} AS status,
                           summary, run_id, created_at AS at FROM task_completions c
                           WHERE task_id=? AND status IN ('done','produced_unverified','failed','cancelled')
                           ORDER BY id DESC LIMIT 1"))
            .bind(task_id).fetch_optional(&self.pool).await?)
    }
    pub async fn pending(&self) -> Result<Vec<CompletionRow>> {
        Ok(sqlx::query_as("SELECT id,task_id,session_id,title,is_goal,status,reason,notify_target FROM task_completions WHERE notify_requested=1 AND enqueued_at IS NULL ORDER BY id LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    pub async fn enqueued(&self, id: i64) -> Result<()> {
        sqlx::query("UPDATE task_completions SET enqueued_at=datetime('now') WHERE id=? AND enqueued_at IS NULL")
            .bind(id).execute(&self.pool).await?;
        Ok(())
    }
}
