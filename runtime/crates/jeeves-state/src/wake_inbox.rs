//! Wake inbox (migration 0005). One row per wake source; the unique `source_key` makes a repeated
//! wake a no-op. `reconcile` rows are never redelivered: a human decides what happened.

use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    Queued,
    Duplicate,
}

#[derive(Debug, Clone, FromRow)]
pub struct WakeRow {
    pub source_key: String,
    pub target_task: String,
    pub payload: String,
    pub state: String,
}

#[derive(Clone)]
pub struct WakeInbox {
    pool: SqlitePool,
}

impl WakeInbox {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Insert-or-ignore. The caller must report `Duplicate`; it is never silent.
    pub async fn wake(
        &self,
        source_key: &str,
        target_task: &str,
        payload: &str,
    ) -> Result<WakeOutcome> {
        let n = sqlx::query(
            "INSERT OR IGNORE INTO wake_inbox (source_key, target_task, payload) VALUES (?, ?, ?)",
        )
        .bind(source_key)
        .bind(target_task)
        .bind(payload)
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(if n == 1 {
            WakeOutcome::Queued
        } else {
            WakeOutcome::Duplicate
        })
    }

    pub async fn pending(&self) -> Result<Vec<WakeRow>> {
        self.in_state("pending").await
    }

    pub async fn needs_reconcile(&self) -> Result<Vec<WakeRow>> {
        self.in_state("reconcile").await
    }

    async fn in_state(&self, state: &str) -> Result<Vec<WakeRow>> {
        Ok(sqlx::query_as(
            "SELECT source_key, target_task, payload, state FROM wake_inbox WHERE state = ? ORDER BY created_at, rowid",
        )
        .bind(state)
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn delivered(&self, source_key: &str) -> Result<()> {
        self.settle(source_key, "delivered").await
    }

    /// Claimed but unsettled: effects may have happened, so it is not redelivered.
    pub async fn reconcile(&self, source_key: &str) -> Result<()> {
        self.settle(source_key, "reconcile").await
    }

    async fn settle(&self, source_key: &str, state: &str) -> Result<()> {
        sqlx::query(
            "UPDATE wake_inbox SET state = ?, settled_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE source_key = ? AND state = 'pending'",
        )
        .bind(state)
        .bind(source_key)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "wake_inbox_tests.rs"]
mod wake_inbox_tests;
