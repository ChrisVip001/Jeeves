//! Scheduled-job declarations (migration 0001; `run_at` from 0016): what to run, when, and where the result goes.
//! The runtime half (last fired, claimed slot) stays in `schedules.rs`.

use sqlx::SqlitePool;

/// One declared job.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ScheduleJobRow {
    pub name: String,
    pub cron: String,
    pub prompt: String,
    pub delivery: String,
    pub catch_up: bool,
    /// Local `YYYY-MM-DD HH:MM` for a one-shot job (migration 0016); `None` repeats on `cron`.
    pub run_at: Option<String>,
}

#[derive(Clone)]
pub struct ScheduleJobs {
    pool: SqlitePool,
}

impl ScheduleJobs {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Adds or replaces one job (the name is the key).
    pub async fn save(&self, job: &ScheduleJobRow) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO schedule_jobs (name, cron, prompt, delivery, catch_up, run_at) VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(name) DO UPDATE SET cron = excluded.cron, prompt = excluded.prompt, \
             delivery = excluded.delivery, catch_up = excluded.catch_up, run_at = excluded.run_at",
        )
        .bind(&job.name)
        .bind(&job.cron)
        .bind(&job.prompt)
        .bind(&job.delivery)
        .bind(job.catch_up)
        .bind(&job.run_at)
        .execute(&self.pool)
        .await
        .map(|_| ())
    }

    /// Inserts a job only when the name is free (the import path must never overwrite a job that
    /// was edited after the first import).
    pub async fn insert_if_absent(&self, job: &ScheduleJobRow) -> Result<bool, sqlx::Error> {
        let done = sqlx::query(
            "INSERT OR IGNORE INTO schedule_jobs (name, cron, prompt, delivery, catch_up, run_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&job.name)
        .bind(&job.cron)
        .bind(&job.prompt)
        .bind(&job.delivery)
        .bind(job.catch_up)
        .bind(&job.run_at)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Deletes a one-shot job only if it is still the same one-shot time: a person who re-saved the
    /// name for another time (or as a repeating job) keeps the new declaration.
    pub async fn remove_one_shot(&self, name: &str, run_at: &str) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM schedule_jobs WHERE name = ? AND run_at = ?")
            .bind(name)
            .bind(run_at)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// Deletes one job; `false` when there was none.
    pub async fn remove(&self, name: &str) -> Result<bool, sqlx::Error> {
        let done = sqlx::query("DELETE FROM schedule_jobs WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// All jobs, sorted by name.
    pub async fn list(&self) -> Result<Vec<ScheduleJobRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT name, cron, prompt, delivery, catch_up, run_at FROM schedule_jobs ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
    }
}
