//! Where a task was dispatched from (a chat group message), so its report can go back there.
//! Tasks started from a local view have no row.

use anyhow::Result;
use sqlx::SqlitePool;

/// One source row. Plain strings: the protocol type lives in `jeeves-protocol`, which the state
/// crate does not depend on.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TaskSource {
    pub channel: String,
    pub chat_id: String,
    pub message_id: String,
    pub sender_id: String,
}

/// Record the source of a task. Re-recording replaces it (a retried dispatch of the same task).
pub async fn set(pool: &SqlitePool, task_id: &str, source: &TaskSource) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO task_sources (task_id, channel, chat_id, message_id, sender_id)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(task_id)
    .bind(&source.channel)
    .bind(&source.chat_id)
    .bind(&source.message_id)
    .bind(&source.sender_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get(pool: &SqlitePool, task_id: &str) -> Result<Option<TaskSource>> {
    Ok(sqlx::query_as::<_, TaskSource>(
        "SELECT channel, chat_id, message_id, sender_id FROM task_sources WHERE task_id = ?",
    )
    .bind(task_id)
    .fetch_optional(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_source_round_trips_and_is_absent_for_local_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("state.db")).await.unwrap();
        let source = TaskSource {
            channel: "lark".into(),
            chat_id: "oc_1".into(),
            message_id: "om_1".into(),
            sender_id: "ou_1".into(),
        };
        set(&pool, "t1", &source).await.unwrap();
        assert_eq!(get(&pool, "t1").await.unwrap(), Some(source));
        assert_eq!(get(&pool, "local").await.unwrap(), None);
    }
}
