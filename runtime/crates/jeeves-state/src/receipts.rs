//! Receipts (migration 0004): "the tool said it worked" and "a read-back confirmed it" are two
//! different facts. Only the second counts for `Acceptance::ReceiptConfirmed`.

use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct ReceiptRow {
    pub id: i64,
    pub task_id: String,
    pub run_id: Option<String>,
    pub kind: String,
    #[sqlx(rename = "ref")]
    pub reference: String,
    pub status: String,
    pub evidence_path: Option<String>,
    pub observed_at: String,
}

#[derive(Clone)]
pub struct Receipts {
    pool: SqlitePool,
}

impl Receipts {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The tool reported success. Returns the receipt id to confirm or fail later.
    pub async fn claim(
        &self,
        task_id: &str,
        run_id: Option<&str>,
        kind: &str,
        reference: &str,
    ) -> Result<i64> {
        Ok(sqlx::query(
            "INSERT INTO receipts (task_id, run_id, kind, ref, status) VALUES (?, ?, ?, ?, 'claimed')",
        )
        .bind(task_id)
        .bind(run_id)
        .bind(kind)
        .bind(reference)
        .execute(&self.pool)
        .await?
        .last_insert_rowid())
    }

    /// A read-back saw the effect. `evidence_path` points at what was read.
    pub async fn confirm(&self, id: i64, evidence_path: Option<&str>) -> Result<()> {
        self.settle(id, "confirmed", evidence_path).await
    }

    /// A read-back did not see the effect.
    pub async fn fail(&self, id: i64, evidence_path: Option<&str>) -> Result<()> {
        self.settle(id, "failed", evidence_path).await
    }

    async fn settle(&self, id: i64, status: &str, evidence_path: Option<&str>) -> Result<()> {
        sqlx::query(
            "UPDATE receipts SET status = ?, evidence_path = ?,
             observed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?",
        )
        .bind(status)
        .bind(evidence_path)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list(&self, task_id: &str) -> Result<Vec<ReceiptRow>> {
        Ok(sqlx::query_as::<_, ReceiptRow>(
            "SELECT id, task_id, run_id, kind, ref, status, evidence_path, observed_at
             FROM receipts WHERE task_id = ? ORDER BY id",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?)
    }

    /// `(confirmed, claimed-only)` counts for receipts of `kind` whose ref starts with `prefix`.
    pub async fn count_for(&self, task_id: &str, kind: &str, prefix: &str) -> Result<(i64, i64)> {
        let rows = self.list(task_id).await?;
        let mut confirmed = 0;
        let mut claimed = 0;
        for row in rows
            .iter()
            .filter(|r| r.kind == kind && r.reference.starts_with(prefix))
        {
            match row.status.as_str() {
                "confirmed" => confirmed += 1,
                "claimed" => claimed += 1,
                _ => {}
            }
        }
        Ok((confirmed, claimed))
    }
}
