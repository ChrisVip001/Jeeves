//! Transfer the configured local-date clock to the responsibility ledger.
use super::super::{Delivery, Job};
use crate::Server;
use anyhow::{ensure, Context, Result};
use chrono::{NaiveDateTime, TimeZone};
use jeeves_state::responsibility_runs as runs;

pub(in crate::scheduler) async fn bind(
    conn: &mut sqlx::SqliteConnection,
    briefing: super::Briefing,
) -> Result<bool> {
    let job = briefing.job();
    let delivery = if job.delivery == Delivery::Notify {
        "notify"
    } else {
        "outbox"
    };
    let purpose = format!("Morning briefing · {} · {}", briefing.zone, delivery);
    let plan = serde_json::json!({
        "trigger":{"kind":"cron","expression":job.cron,"timeZone":briefing.zone.to_string()},
        "action":{"kind":"morningBriefing","version":1,"config":{"timeZone":briefing.zone.to_string(),"notify":job.delivery==Delivery::Notify}},
        "delivery":{"kind":"draft"}
    }).to_string();
    sqlx::query("INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,enabled_ms,clock_owner,plan_json) VALUES(?,?,?, ?,253402300799000,?,0,'morning',?) ON CONFLICT(id) DO UPDATE SET purpose=excluded.purpose,plan_json=excluded.plan_json,revision=revision+1 WHERE clock_owner='morning' AND (purpose<>excluded.purpose OR plan_json IS NULL)")
        .bind(super::NAME).bind(purpose).bind(r#"["calendar","unreadMail","openTasks"]"#).bind(job.cron).bind(r#"{"tools":4}"#).bind(plan).execute(&mut *conn).await?;
    let (owner, status): (String, String) =
        sqlx::query_as("SELECT clock_owner,status FROM responsibilities WHERE id=?")
            .bind(super::NAME)
            .fetch_one(conn)
            .await?;
    ensure!(
        owner == "morning",
        "Morning briefing ID belongs to an existing responsibility"
    );
    Ok(status == "active")
}

/// Never enter the model runner or its retrying notification queue for this duty.
pub(in crate::scheduler) async fn fire(
    server: &Server,
    job: &Job,
    briefing: super::Briefing,
    at: NaiveDateTime,
    minute: &str,
) -> Result<()> {
    let key = format!("schedule:{}:{minute}", job.name);
    let wake_key = key.as_str();
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    let active = bind(&mut tx, briefing).await?;
    if !active || !jeeves_state::Schedules::claim_once_on(&mut tx, &job.name, minute).await? {
        eprintln!("(jeeves) morning briefing paused or slot already handled: {minute}");
        return Ok(());
    }
    let slot = chrono::NaiveDateTime::parse_from_str(minute, "%Y-%m-%dT%H:%M")?;
    let instant = briefing
        .zone
        .from_local_datetime(&slot)
        .earliest()
        .context("Morning slot does not exist")?;
    runs::enqueue_bound(
        &mut tx,
        super::NAME,
        wake_key,
        instant.timestamp_millis(),
        Some(&at.format("%Y-%m-%dT%H:%M:%S").to_string()),
    )
    .await?;
    tx.commit().await?;
    let Some(work) = runs::take_key(
        &server.pool,
        chrono::Utc::now().timestamp_millis(),
        Some(wake_key),
    )
    .await?
    else {
        return Ok(());
    };
    let (sink, _rx) = crate::EventSink::channel();
    crate::responsibility::worker::process(server, &sink, &work).await?;
    let detail: Option<String> = sqlx::query_scalar(
        "SELECT detail FROM responsibility_runs WHERE wake_key=? AND status='review'",
    )
    .bind(wake_key)
    .fetch_optional(&server.pool)
    .await?
    .flatten();
    if let Some(detail) = detail {
        anyhow::bail!("Morning briefing requires review; delivery was not retried: {detail}");
    }
    Ok(())
}
