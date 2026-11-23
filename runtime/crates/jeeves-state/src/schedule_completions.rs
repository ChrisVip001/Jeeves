//! Settle a trigger and produce its outcome/notification receipt in one transaction.
use anyhow::{Result, ensure};
use sqlx::{FromRow, SqlitePool};

pub struct ScheduleCompletion<'a> {
    pub name: &'a str,
    pub minute: &'a str,
    pub status: &'a str,
    pub detail: &'a str,
    pub output: &'a str,
    pub preview: &'a str,
    pub output_path: Option<&'a str>,
    pub notify_requested: bool,
    pub notify_target: Option<&'a str>,
}
#[derive(Debug, FromRow)]
pub struct ScheduleCompletionRow {
    pub name: String,
    pub minute: String,
    pub status: String,
    pub detail: String,
    pub preview: String,
    pub notify_target: Option<String>,
}
#[derive(Debug, FromRow)]
pub struct ScheduleOutputRow {
    pub name: String,
    pub minute: String,
    pub output_path: String,
    pub output: String,
}
#[derive(Clone)]
pub struct ScheduleCompletions {
    pool: SqlitePool,
}
impl ScheduleCompletions {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    pub async fn settle(&self, row: ScheduleCompletion<'_>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        Self::settle_on(&mut tx, row).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn settle_on(
        conn: &mut sqlx::SqliteConnection,
        row: ScheduleCompletion<'_>,
    ) -> Result<()> {
        ensure!(
            matches!(row.status, "produced" | "empty" | "needs_attention"),
            "Invalid schedule outcome"
        );
        let changed = sqlx::query(
            "UPDATE schedule_state SET
            last_fired_minute=CASE WHEN ?='needs_attention' THEN last_fired_minute ELSE ? END,
            claimed_minute=CASE WHEN ?='needs_attention' THEN claimed_minute ELSE NULL END,
            updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE name=? AND claimed_minute=?",
        )
        .bind(row.status)
        .bind(row.minute)
        .bind(row.status)
        .bind(row.name)
        .bind(row.minute)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if changed == 0 {
            let status: Option<String> = sqlx::query_scalar(
                "SELECT status FROM schedule_completions WHERE name=? AND minute=?",
            )
            .bind(row.name)
            .bind(row.minute)
            .fetch_optional(&mut *conn)
            .await?;
            ensure!(
                status.as_deref() == Some(row.status),
                "No matching schedule claim or outcome"
            );
        } else {
            let inserted=sqlx::query("INSERT INTO schedule_completions(name,minute,status,detail,output,preview,output_path,notify_requested,notify_target)
                VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT DO NOTHING")
                .bind(row.name).bind(row.minute).bind(row.status).bind(row.detail).bind(row.output).bind(row.preview.chars().take(120).collect::<String>()).bind(row.output_path).bind(row.notify_requested).bind(row.notify_target)
                .execute(&mut *conn).await?.rows_affected();
            if inserted == 0 {
                let status: String = sqlx::query_scalar(
                    "SELECT status FROM schedule_completions WHERE name=? AND minute=?",
                )
                .bind(row.name)
                .bind(row.minute)
                .fetch_one(&mut *conn)
                .await?;
                ensure!(
                    status == row.status,
                    "Schedule outcome is immutable; human review is required"
                );
            }
        }
        Ok(())
    }
    pub async fn pending(&self) -> Result<Vec<ScheduleCompletionRow>> {
        Ok(sqlx::query_as("SELECT name,minute,status,detail,preview,notify_target FROM schedule_completions WHERE notify_requested=1 AND enqueued_at IS NULL AND (output_path IS NULL OR output_written_at IS NOT NULL) ORDER BY rowid LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    pub async fn pending_outputs(&self) -> Result<Vec<ScheduleOutputRow>> {
        Ok(sqlx::query_as("SELECT name,minute,output_path,output FROM schedule_completions WHERE output_path IS NOT NULL AND output_written_at IS NULL ORDER BY rowid LIMIT 100")
            .fetch_all(&self.pool).await?)
    }
    pub async fn output_written(&self, name: &str, minute: &str) -> Result<()> {
        sqlx::query("UPDATE schedule_completions SET output_written_at=datetime('now') WHERE name=? AND minute=?")
            .bind(name).bind(minute).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn output_delivered(&self, name: &str, minute: &str) -> Result<bool> {
        Ok(sqlx::query_scalar("SELECT output_written_at IS NOT NULL FROM schedule_completions WHERE name=? AND minute=?")
            .bind(name).bind(minute).fetch_optional(&self.pool).await?.unwrap_or(false))
    }
    pub async fn enqueued(&self, name: &str, minute: &str) -> Result<()> {
        sqlx::query("UPDATE schedule_completions SET enqueued_at=datetime('now') WHERE name=? AND minute=? AND enqueued_at IS NULL")
            .bind(name).bind(minute).execute(&self.pool).await?;
        Ok(())
    }
    /// Explicit human acknowledgement clears the old claim; it never replays its turn.
    pub async fn acknowledge(&self, name: &str) -> Result<Option<String>> {
        let mut tx = self.pool.begin().await?;
        let minute: Option<String> = sqlx::query_scalar(
            "UPDATE schedule_state SET last_fired_minute=claimed_minute, claimed_minute=NULL,
             updated_at=strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE name=? AND claimed_minute IS NOT NULL
             RETURNING last_fired_minute",
        ).bind(name).fetch_optional(&mut *tx).await?;
        let Some(minute) = minute else {
            return Ok(None);
        };
        sqlx::query("INSERT INTO schedule_completions(name,minute,status,detail,output,preview,notify_requested,reviewed_at)
            VALUES(?,?,'reviewed','Human reviewed the interrupted trigger; no replay requested','','',0,datetime('now')) ON CONFLICT(name,minute) DO UPDATE SET reviewed_at=datetime('now')")
            .bind(name).bind(&minute).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Some(minute))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn acknowledgement_waits_for_a_concurrent_writer_and_settles_once() {
        let home = tempfile::tempdir().unwrap();
        let pool = crate::open(&home.path().join("state.db")).await.unwrap();
        let schedules = crate::Schedules::new(pool.clone());
        schedules.ensure("job", true, "outbox").await.unwrap();
        schedules.claim("job", "2026-11-19T08:00").await.unwrap();
        let mut writer = pool.begin().await.unwrap();
        sqlx::query("UPDATE schedule_state SET delivery='outbox' WHERE name='job'")
            .execute(&mut *writer)
            .await
            .unwrap();
        let rows = ScheduleCompletions::new(pool);
        let release = async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            writer.commit().await.unwrap();
        };
        let (first, ()) = tokio::join!(rows.acknowledge("job"), release);
        assert_eq!(first.unwrap().as_deref(), Some("2026-11-19T08:00"));
        assert_eq!(rows.acknowledge("job").await.unwrap(), None);
    }
}
