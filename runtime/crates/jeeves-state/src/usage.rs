//! Aggregate provider-reported request usage and historical turn-only records.

use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

/// One aggregated row per model.
#[derive(Debug, Clone, FromRow)]
pub struct UsageByModel {
    pub provider: String,
    pub model: String,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Full summary.
#[derive(Debug, Clone, Default)]
pub struct UsageSummary {
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub by_model: Vec<UsageByModel>,
}

/// The usage ledger. Clone is cheap (one pool handle), same reason as Jobs.
#[derive(Clone)]
pub struct Usage {
    pool: SqlitePool,
}

impl Usage {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Charge reported requests once, including reports persisted before a process stop.
    pub async fn charge_pending(&self, task: Option<&str>, run: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("UPDATE model_requests SET charged=1 WHERE charged=0 AND input_tokens IS NOT NULL AND ((?1 IS NULL AND ?2 IS NULL) OR task_id=COALESCE(?1,(SELECT task_id FROM task_runs WHERE id=?2)))")
            .bind(task).bind(run).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    #[cfg(test)]
    async fn record_legacy(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO historical_turn_usage (session_id, provider, model, input_tokens, output_tokens)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(provider)
        .bind(model)
        .bind(input_tokens)
        .bind(output_tokens)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Full summary over all time (per-model aggregates + totals), by_model sorted by total input desc.
    pub async fn summary(&self) -> Result<UsageSummary> {
        self.summary_between(None, None).await
    }

    /// The same summary for one period: `since` inclusive, `until` exclusive, either may be absent.
    /// Both are UTC, `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS` — the format `created_at` is stored in,
    /// so string order is time order and a billing period is just two dates.
    pub async fn summary_between(
        &self,
        since: Option<&str>,
        until: Option<&str>,
    ) -> Result<UsageSummary> {
        let rows = sqlx::query_as::<_, (bool, String, String, i64, i64, i64)>(
            "WITH entries AS (
                SELECT provider,model,created_at,input_tokens,output_tokens,'legacy:'||id AS owner FROM historical_turn_usage
                UNION ALL SELECT provider,model,created_at,input_tokens,output_tokens,'turn:'||turn_id FROM model_requests
             ), period AS (
                SELECT * FROM entries WHERE (?1 IS NULL OR created_at >= ?1) AND (?2 IS NULL OR created_at < ?2)
             )
             SELECT 0 AS overall, provider, model, COUNT(DISTINCT owner), COALESCE(SUM(input_tokens),0) AS input, COALESCE(SUM(output_tokens),0) FROM period GROUP BY provider,model
             UNION ALL SELECT 1,'','',COUNT(DISTINCT owner), COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0) FROM period
             ORDER BY input DESC,provider,model",
        )
        .bind(since).bind(until).fetch_all(&self.pool).await?;
        let mut summary = UsageSummary::default();
        for (overall, provider, model, turns, input_tokens, output_tokens) in rows {
            if overall {
                summary.turns = turns;
                summary.input_tokens = input_tokens;
                summary.output_tokens = output_tokens;
            } else {
                summary.by_model.push(UsageByModel {
                    provider,
                    model,
                    turns,
                    input_tokens,
                    output_tokens,
                });
            }
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (Usage, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("temp dir");
        let pool = crate::open(&dir.path().join("state.db"))
            .await
            .expect("open the store");
        (Usage::new(pool), dir)
    }

    /// What goes in aggregates out; by_model sorted by input desc; empty table returns zeros.
    #[tokio::test]
    async fn records_and_aggregates_by_model() {
        let (usage, _dir) = db().await;
        let empty = usage.summary().await.expect("empty summary");
        assert_eq!(empty.turns, 0);

        usage
            .record_legacy("s1", "deepseek", "deepseek-flash", 100, 50)
            .await
            .expect("record one row");
        usage
            .record_legacy("s1", "deepseek", "deepseek-flash", 200, 80)
            .await
            .expect("record another row");
        usage
            .record_legacy("s2", "openai", "gpt-5.2", 1000, 10)
            .await
            .expect("another vendor");

        let summary = usage.summary().await.expect("summary");
        assert_eq!(summary.turns, 3);
        assert_eq!(summary.input_tokens, 1300);
        assert_eq!(summary.output_tokens, 140);
        assert_eq!(summary.by_model.len(), 2);
        assert_eq!(summary.by_model[0].model, "gpt-5.2");
        assert_eq!(summary.by_model[0].turns, 1);
        assert_eq!(summary.by_model[1].model, "deepseek-flash");
        assert_eq!(summary.by_model[1].turns, 2);
        assert_eq!(summary.by_model[1].input_tokens, 300);
    }

    #[tokio::test]
    async fn a_routing_request_and_a_different_execution_model_share_one_turn_and_budget() {
        let (usage, _dir) = db().await;
        let pool = &usage.pool;
        sqlx::query(
            "INSERT INTO agent_tasks(id,title,budget_json) VALUES('t','task','{\"tokens\":20}')",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO model_requests(session_id,turn_id,task_id,purpose,provider,model,termination,input_tokens,output_tokens) VALUES('s','turn','t','routing','p','cheap','end_turn',5,2)").execute(pool).await.unwrap();
        usage.charge_pending(Some("t"), None).await.unwrap();
        crate::Runs::new(pool.clone())
            .start("run", "t", Some("s"), Some("turn"))
            .await
            .unwrap();
        sqlx::query("INSERT INTO model_requests(session_id,turn_id,task_id,run_id,purpose,provider,model,termination,input_tokens,output_tokens) VALUES('s','turn','t','run','execution','p','main','end_turn',10,3)").execute(pool).await.unwrap();
        usage.charge_pending(None, Some("run")).await.unwrap();
        usage.charge_pending(None, None).await.unwrap();
        let spent: (i64,i64,i64) = sqlx::query_as("SELECT t.spent_tokens,r.tokens_in,r.tokens_out FROM agent_tasks t JOIN task_runs r ON r.task_id=t.id").fetch_one(pool).await.unwrap();
        assert_eq!(spent, (20, 15, 5));
        let summary = usage.summary().await.unwrap();
        assert_eq!(
            (
                summary.turns,
                summary.by_model.len(),
                summary.input_tokens,
                summary.output_tokens
            ),
            (1, 2, 15, 5)
        );
        assert!(sqlx::query("INSERT INTO model_requests(session_id,turn_id,task_id,purpose,provider,model) VALUES('s','screen','t','screen','p','vision')").execute(pool).await.unwrap_err().to_string().contains("budget exhausted"));
    }

    /// A period is half-open: `since` is inside, `until` is outside, and a missing end is open.
    #[tokio::test]
    async fn a_period_only_counts_rows_inside_it() {
        let (usage, _dir) = db().await;
        for (at, input) in [
            ("2026-10-31 23:59:59", 1),
            ("2026-11-01 00:00:00", 10),
            ("2026-11-30 23:59:59", 100),
            ("2026-12-01 00:00:00", 1000),
        ] {
            sqlx::query("INSERT INTO historical_turn_usage (session_id, provider, model, input_tokens, output_tokens, created_at) VALUES ('s', 'p', 'm', ?, 0, ?)")
                .bind(input)
                .bind(at)
                .execute(&usage.pool)
                .await
                .expect("insert a dated row");
        }
        let november = usage
            .summary_between(Some("2026-11-01"), Some("2026-12-01"))
            .await
            .expect("period summary");
        assert_eq!(november.turns, 2);
        assert_eq!(november.input_tokens, 110);
        let open_ended = usage
            .summary_between(Some("2026-11-30"), None)
            .await
            .expect("open-ended summary");
        assert_eq!(open_ended.input_tokens, 1100);
        assert_eq!(usage.summary().await.expect("all time").turns, 4);
    }
}
