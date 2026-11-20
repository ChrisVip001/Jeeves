//! Usage ledger (migration 0010): bookkeeping only, never enforcement.
//!
//! One row per turn; `summary` computes per-model aggregates in SQL. Deliberately no cost
//! column — under BYOK the money is on the user's own vendor bill; rates are the deployer's
//! business (tokens are stored; rates can be applied anytime). One home, one state.db, one
//! owner: there is nobody else's usage to keep apart.

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

    /// Record one turn's usage. Write failure **never blocks the turn** — bookkeeping is a side path.
    pub async fn record(
        &self,
        session_id: &str,
        provider: &str,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO usage_ledger (session_id, provider, model, input_tokens, output_tokens)
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
        let by_model = sqlx::query_as::<_, UsageByModel>(
            "SELECT provider, model, COUNT(*) AS turns,
                    SUM(input_tokens) AS input_tokens,
                    SUM(output_tokens) AS output_tokens
             FROM usage_ledger
             WHERE (?1 IS NULL OR created_at >= ?1) AND (?2 IS NULL OR created_at < ?2)
             GROUP BY provider, model
             ORDER BY SUM(input_tokens) DESC",
        )
        .bind(since)
        .bind(until)
        .fetch_all(&self.pool)
        .await?;
        let summary = UsageSummary {
            turns: by_model.iter().map(|row| row.turns).sum(),
            input_tokens: by_model.iter().map(|row| row.input_tokens).sum(),
            output_tokens: by_model.iter().map(|row| row.output_tokens).sum(),
            by_model,
        };
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
            .record("s1", "deepseek", "deepseek-flash", 100, 50)
            .await
            .expect("record one row");
        usage
            .record("s1", "deepseek", "deepseek-flash", 200, 80)
            .await
            .expect("record another row");
        usage
            .record("s2", "openai", "gpt-5.2", 1000, 10)
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
            sqlx::query("INSERT INTO usage_ledger (session_id, provider, model, input_tokens, output_tokens, created_at) VALUES ('s', 'p', 'm', ?, 0, ?)")
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
