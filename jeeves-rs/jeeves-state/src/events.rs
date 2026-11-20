//! The task event log (migration 0017).
//!
//! Not a second source of truth — the task row is. This is the **readable timeline** behind it:
//! every verification round, every budget refusal, every "the tool said it wrote, the file is not
//! there". Written in one INSERT per event; nobody ever has to reconstruct history from prose.

use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

/// One event.
#[derive(Debug, Clone, FromRow)]
pub struct EventRow {
    pub id: i64,
    pub task_id: String,
    pub run_id: Option<String>,
    /// Monotonic per task (1, 2, 3 …) — the order a human reads on the timeline.
    pub seq: i32,
    /// `run_started` / `verify` / `budget` / `contradiction` / `stopped` / `artifact` …
    pub kind: String,
    /// Human-readable detail. Never a file body — a path, a count, a reason.
    pub detail: Option<String>,
    pub at: String,
}

/// The event store. Clone is cheap (one pool handle).
#[derive(Clone)]
pub struct Events {
    pool: SqlitePool,
}

impl Events {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Append one event. `seq` is computed inside the INSERT so two writers cannot both claim
    /// the same number (the pool may hand out several connections).
    pub async fn append(
        &self,
        task_id: &str,
        run_id: Option<&str>,
        kind: &str,
        detail: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO task_events (task_id, run_id, seq, kind, detail)
             VALUES (?, ?, (SELECT COALESCE(MAX(seq), 0) + 1 FROM task_events WHERE task_id = ?), ?, ?)",
        )
        .bind(task_id)
        .bind(run_id)
        .bind(task_id)
        .bind(kind)
        .bind(detail)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Newest first, capped — the timeline panel reads at most a screenful.
    pub async fn list(&self, task_id: &str, limit: i64) -> Result<Vec<EventRow>> {
        Ok(sqlx::query_as::<_, EventRow>(
            "SELECT id, task_id, run_id, seq, kind, detail, at FROM task_events
             WHERE task_id = ? ORDER BY seq DESC, id DESC LIMIT ?",
        )
        .bind(task_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// Events after `cursor` (a row `id`; 0 = from the start), oldest first, at most `limit`
    /// (capped at [`MAX_SINCE`]). Returns the rows and the cursor to resume from: the last
    /// row's id, or `cursor` itself when nothing is newer. `task_id` narrows to one task.
    pub async fn since(
        &self,
        cursor: i64,
        task_id: Option<&str>,
        limit: i32,
    ) -> Result<(Vec<EventRow>, i64)> {
        let limit = i64::from(limit.clamp(1, MAX_SINCE));
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT id, task_id, run_id, seq, kind, detail, at FROM task_events
             WHERE id > ? AND (? IS NULL OR task_id = ?) ORDER BY id ASC LIMIT ?",
        )
        .bind(cursor)
        .bind(task_id)
        .bind(task_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        let next = rows.last().map_or(cursor, |row| row.id);
        Ok((rows, next))
    }

    /// The newest event id, optionally for one task: the cursor a "knock" carries. 0 when empty.
    pub async fn latest_cursor(&self, task_id: Option<&str>) -> Result<i64> {
        let id: Option<i64> =
            sqlx::query_scalar("SELECT MAX(id) FROM task_events WHERE (? IS NULL OR task_id = ?)")
                .bind(task_id)
                .bind(task_id)
                .fetch_one(&self.pool)
                .await?;
        Ok(id.unwrap_or(0))
    }
}

/// Most events one `since` call returns.
pub const MAX_SINCE: i32 = 500;
