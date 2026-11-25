//! Bind a model tool call to the live task run that owns it.
use anyhow::{Result, anyhow};
use jeeves_tools::ToolCtx;
use sqlx::SqlitePool;

/// Resolve the task for a model tool's run id. The SQL is the single definition of "active"
/// for model-driven browser/computer tools: the run must exist, belong to this session and
/// its task must still be running. RPC handlers use a different admission rule and do not
/// call this function.
pub(crate) async fn bound_active_task(
    pool: &SqlitePool,
    ctx: &ToolCtx,
    tool_label: &'static str,
) -> Result<String> {
    let run = ctx
        .task_run_id
        .as_deref()
        .ok_or_else(|| anyhow!("{tool_label} tools require an independent task execution"))?;
    let task: Option<String> = sqlx::query_scalar(
        "SELECT r.task_id FROM task_runs r JOIN agent_tasks t ON t.id=r.task_id WHERE r.id=? AND r.session_id=? AND t.status='running'",
    )
    .bind(run)
    .bind(&ctx.session_id)
    .fetch_optional(pool)
    .await?;
    task.ok_or_else(|| anyhow!("{tool_label} execution no longer belongs to an active task"))
}
