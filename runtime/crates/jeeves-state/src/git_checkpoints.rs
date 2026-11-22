//! Durable human checkpoint policy and exact execution-boundary to commit mapping.
use anyhow::Result;
use sqlx::SqlitePool;

pub async fn policy(pool: &SqlitePool, task: &str) -> Result<Option<Vec<String>>> {
    let row: Option<String> =
        sqlx::query_scalar("SELECT paths_json FROM task_git_checkpoint_policies WHERE task_id=?")
            .bind(task)
            .fetch_optional(pool)
            .await?;
    row.map(|r| serde_json::from_str(&r).map_err(Into::into))
        .transpose()
}
pub async fn set_policy(pool: &SqlitePool, task: &str, paths: Option<&[String]>) -> Result<()> {
    if let Some(paths) = paths {
        sqlx::query("INSERT INTO task_git_checkpoint_policies(task_id,paths_json) VALUES(?,?) ON CONFLICT(task_id) DO UPDATE SET paths_json=excluded.paths_json").bind(task).bind(serde_json::to_string(paths)?).execute(pool).await?;
    } else {
        sqlx::query("DELETE FROM task_git_checkpoint_policies WHERE task_id=?")
            .bind(task)
            .execute(pool)
            .await?;
    }
    Ok(())
}
pub async fn boundary(
    pool: &SqlitePool,
    task: &str,
    turn: &str,
    digest: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT commit_sha FROM task_git_checkpoint_boundaries WHERE task_id=? AND turn_id=? AND boundary_digest=?").bind(task).bind(turn).bind(digest).fetch_optional(pool).await?)
}
pub async fn record(
    pool: &SqlitePool,
    task: &str,
    turn: &str,
    digest: &str,
    sha: &str,
) -> Result<bool> {
    let inserted=sqlx::query("INSERT OR IGNORE INTO task_git_checkpoint_boundaries(task_id,turn_id,boundary_digest,commit_sha) VALUES(?,?,?,?)").bind(task).bind(turn).bind(digest).bind(sha).execute(pool).await?.rows_affected()==1;
    Ok(inserted)
}
