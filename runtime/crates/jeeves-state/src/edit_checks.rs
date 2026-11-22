//! A task's `after: edit` checks, stored beside the task (migration 0006).
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct EditChecks {
    pool: SqlitePool,
}

impl EditChecks {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn set(&self, task_id: &str, checks_json: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO task_edit_checks (task_id, checks_json) VALUES (?, ?)
             ON CONFLICT(task_id) DO UPDATE SET checks_json = excluded.checks_json",
        )
        .bind(task_id)
        .bind(checks_json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get(&self, task_id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT checks_json FROM task_edit_checks WHERE task_id = ?")
                .bind(task_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn checks_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
        let store = EditChecks::new(pool);
        assert!(store.get("t").await.unwrap().is_none());
        store.set("t", "[1]").await.unwrap();
        store.set("t", "[2]").await.unwrap();
        assert_eq!(store.get("t").await.unwrap().as_deref(), Some("[2]"));
    }
}
