//! Human-owned recurring duties. The claim and its wake are one SQLite commit.
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow)]
pub struct ResponsibilityRow {
    pub clock_owner: String,
    pub migration_note: Option<String>,
    pub revision: i32,
    pub digest_json: Option<String>,
    pub plan_json: Option<String>,
    pub next_due_ms: Option<i64>,
    pub id: String,
    pub purpose: String,
    pub sources_json: String,
    pub cron: String,
    pub expires_ms: i64,
    pub budget_json: String,
    pub research_task_id: Option<String>,
    pub feishu_recipient: Option<String>,
    pub status: String,
    pub last_slot: Option<i64>,
}

#[derive(Clone, Copy)]
pub struct Responsibilities<'a>(pub &'a SqlitePool);

impl Responsibilities<'_> {
    pub async fn register(&self, row: &ResponsibilityRow, now: i64) -> sqlx::Result<bool> {
        Self::register_on(&mut *self.0.acquire().await?, row, now).await
    }

    pub async fn register_on(
        conn: &mut sqlx::SqliteConnection,
        row: &ResponsibilityRow,
        now: i64,
    ) -> sqlx::Result<bool> {
        let result = sqlx::query(
            "INSERT INTO responsibilities
             (id, purpose, sources_json, cron, expires_ms, budget_json, enabled_ms, research_task_id,feishu_recipient,digest_json,plan_json,next_due_ms)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
        )
        .bind(&row.id)
        .bind(&row.purpose)
        .bind(&row.sources_json)
        .bind(&row.cron)
        .bind(row.expires_ms)
        .bind(&row.budget_json)
        .bind(now)
        .bind(&row.research_task_id).bind(&row.feishu_recipient)
        .bind(&row.digest_json)
        .bind(&row.plan_json)
        .bind(row.next_due_ms)
        .execute(conn)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn amend(
        &self,
        row: &ResponsibilityRow,
        expected: i32,
        now: i64,
    ) -> sqlx::Result<Option<ResponsibilityRow>> {
        let mut tx = self.0.begin().await?;
        let result = Self::amend_on(&mut tx, row, expected, now).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn amend_on(
        conn: &mut sqlx::SqliteConnection,
        row: &ResponsibilityRow,
        expected: i32,
        now: i64,
    ) -> sqlx::Result<Option<ResponsibilityRow>> {
        let updated = sqlx::query_as(
            "UPDATE responsibilities SET purpose=?,sources_json=?,cron=?,expires_ms=?,budget_json=?,research_task_id=?,feishu_recipient=?,digest_json=?,plan_json=?,next_due_ms=?,
             revision=revision+1,migration_note=NULL,enabled_ms=? WHERE clock_owner='cron' AND id=? AND revision=? AND expires_ms>? AND status!='expired' AND (migration_note IS NULL OR ? IS NOT NULL) RETURNING *"
        ).bind(&row.purpose).bind(&row.sources_json).bind(&row.cron).bind(row.expires_ms)
         .bind(&row.budget_json).bind(&row.research_task_id).bind(&row.feishu_recipient).bind(&row.digest_json).bind(&row.plan_json).bind(row.next_due_ms).bind(now).bind(&row.id)
         .bind(expected).bind(now).bind(&row.plan_json).fetch_optional(&mut *conn).await?;
        if updated.is_some() {
            Self::stop_wakes(conn, &row.id).await?;
        }
        Ok(updated)
    }

    pub async fn list(&self) -> sqlx::Result<Vec<ResponsibilityRow>> {
        sqlx::query_as("SELECT * FROM responsibilities ORDER BY id")
            .fetch_all(self.0)
            .await
    }

    pub async fn candidates(
        &self,
        now: i64,
    ) -> sqlx::Result<Vec<(String, String, i32, Option<String>)>> {
        sqlx::query_as(
            "SELECT id, cron, revision, json_extract(digest_json,'$.timeZone') FROM responsibilities
             WHERE clock_owner='cron' AND plan_json IS NULL AND status='active' AND enabled_ms<=? AND (last_slot IS NULL OR last_slot<?)
             ORDER BY id",
        )
        .bind(now)
        .bind(now.div_euclid(60_000))
        .fetch_all(self.0)
        .await
    }

    /// Retain outstanding wakes for review. Resuming cannot deliver a wake from before a stop.
    async fn stop_wakes(tx: &mut sqlx::SqliteConnection, id: &str) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE wake_inbox SET state='reconcile', settled_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE target_task=? AND state='pending'",
        )
        .bind(format!("responsibility:{id}"))
        .execute(tx)
        .await?;
        Ok(())
    }

    pub async fn expire(&self, now: i64) -> sqlx::Result<Vec<String>> {
        let due: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM responsibilities WHERE status!='expired' AND expires_ms<=?)",
        )
        .bind(now)
        .fetch_one(self.0)
        .await?;
        if !due {
            return Ok(Vec::new());
        }
        let mut tx = self.0.begin().await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "UPDATE responsibilities SET status='expired',revision=revision+1
             WHERE status!='expired' AND expires_ms<=? RETURNING id",
        )
        .bind(now)
        .fetch_all(&mut *tx)
        .await?;
        for id in &ids {
            Self::stop_wakes(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(ids)
    }

    pub async fn set_paused(
        &self,
        id: &str,
        paused: bool,
        now: i64,
    ) -> sqlx::Result<Option<(ResponsibilityRow, bool)>> {
        let mut tx = self.0.begin().await?;
        let result = Self::set_paused_on(&mut tx, id, paused, None, now).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn set_paused_on(
        conn: &mut sqlx::SqliteConnection,
        id: &str,
        paused: bool,
        expected: Option<i32>,
        now: i64,
    ) -> sqlx::Result<Option<(ResponsibilityRow, bool)>> {
        let state = if paused { "paused" } else { "active" };
        let result = sqlx::query(
            "UPDATE responsibilities SET status=?, revision=revision+1, enabled_ms=?
             WHERE id=? AND expires_ms>? AND status!=? AND status!='expired' AND (?='paused' OR migration_note IS NULL) AND (? IS NULL OR revision=?)",
        )
        .bind(state)
        .bind(now)
        .bind(id)
        .bind(now)
        .bind(state)
        .bind(state)
        .bind(expected)
        .bind(expected)
        .execute(&mut *conn)
        .await?;
        let changed = result.rows_affected() == 1;
        if paused && changed {
            Self::stop_wakes(conn, id).await?;
        }
        let row = sqlx::query_as("SELECT * FROM responsibilities WHERE id=?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        Ok(row.map(|row| (row, changed)))
    }

    pub async fn claim(&self, id: &str, revision: i32, now: i64) -> sqlx::Result<bool> {
        let slot = now.div_euclid(60_000);
        let mut tx = self.0.begin().await?;
        let purpose: Option<String> = sqlx::query_scalar(
            "UPDATE responsibilities SET last_slot=?
             WHERE clock_owner='cron' AND id=? AND revision=? AND status='active' AND enabled_ms<=? AND expires_ms>?
             AND (last_slot IS NULL OR last_slot<?) RETURNING purpose",
        )
        .bind(slot)
        .bind(id)
        .bind(revision)
        .bind(now)
        .bind(now)
        .bind(slot)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(purpose) = &purpose {
            let target = format!("responsibility:{id}");
            sqlx::query("INSERT INTO wake_inbox(source_key,target_task,payload,responsibility_revision) VALUES(?,?,?,?)")
                .bind(format!("{target}:{slot}"))
                .bind(target)
                .bind(purpose)
                .bind(revision)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(purpose.is_some())
    }
}
