//! Transfer the configured local-date clock to the responsibility ledger.
use super::super::{append_run_log, run_log_line, Delivery, Job};
use crate::Server;
use anyhow::{ensure, Context, Result};
use chrono::{NaiveDateTime, TimeZone};
use jeeves_state::{
    responsibility_runs as runs,
    schedule_completions::{ScheduleCompletion, ScheduleCompletions},
};

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
    sqlx::query("INSERT INTO responsibilities(id,purpose,sources_json,cron,expires_ms,budget_json,enabled_ms,clock_owner) VALUES(?,?,?, ?,253402300799000,?,0,'morning') ON CONFLICT(id) DO UPDATE SET purpose=excluded.purpose,revision=revision+1 WHERE clock_owner='morning' AND purpose<>excluded.purpose")
        .bind(super::NAME).bind(purpose).bind(r#"["calendar","unreadMail","openTasks"]"#).bind(job.cron).bind(r#"{"tools":4}"#).execute(&mut *conn).await?;
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
    let home = server.settings().home;
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
    runs::begin_bound(&mut tx, super::NAME, wake_key, instant.timestamp_millis()).await?;
    tx.commit().await?;
    let started = std::time::Instant::now();
    let proposal = super::proposal(server, at.date(), briefing.zone).await;
    let (body, outcome) = match proposal {
        Ok(body) => {
            runs::proposal(&server.pool, wake_key, &body).await?;
            if !runs::admit_send(
                &server.pool,
                wake_key,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?
            {
                (body, Err("Briefing paused before delivery".into()))
            } else {
                let file_name = format!("schedule-{}-{}.md", job.name, at.format("%Y%m%d-%H%M"));
                let outcome = match crate::schedule_output::write_output(&home, &file_name, &body) {
                    Ok(())
                        if job.delivery == Delivery::Notify
                            && !runs::current_send(
                                &server.pool,
                                wake_key,
                                chrono::Utc::now().timestamp_millis(),
                            )
                            .await? =>
                    {
                        Err("Briefing changed before notification; draft saved locally".into())
                    }
                    Ok(()) if job.delivery == Delivery::Notify => match server.notifier() {
                        Some(notifier) => notifier.send_once(body.clone()).await,
                        None => Err(
                            "selected channel is not configured; the proposal is in outbox".into(),
                        ),
                    },
                    Ok(()) => Ok(()),
                    Err(error) => Err(format!("local proposal delivery failed: {error}")),
                };
                (body, outcome)
            }
        }
        Err(error) => (
            String::new(),
            Err(format!("briefing acceptance failed: {error}")),
        ),
    };
    let (state, report) = match &outcome {
        Ok(()) => ("produced", ""),
        Err(report) => ("needs_attention", report.as_str()),
    };
    let mut tx = server.pool.begin().await?;
    ScheduleCompletions::settle_on(
        &mut tx,
        ScheduleCompletion {
            name: &job.name,
            minute,
            status: state,
            detail: report,
            output: &body,
            preview: report,
            output_path: None,
            notify_requested: false,
            notify_target: None,
        },
    )
    .await?;
    runs::finish_on(
        &mut tx,
        wake_key,
        if outcome.is_err() {
            "review"
        } else if job.delivery == Delivery::Notify {
            "confirmed"
        } else {
            "draft"
        },
        report,
        i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX),
    )
    .await?;
    tx.commit().await?;
    append_run_log(
        &home,
        &run_log_line(
            &job.name,
            minute,
            state,
            report,
            body.chars().count(),
            outcome.is_ok(),
        ),
    );
    if let Err(error) = outcome {
        anyhow::bail!("Morning briefing requires review; delivery was not retried: {error}");
    }
    eprintln!("(jeeves) morning briefing delivered at {minute}");
    Ok(())
}
