//! Run and settle the configured morning briefing using the responsibility worker.
use super::action::{Adapter, Output, Settlement};
use crate::scheduler::{
    append_run_log,
    morning::{self, NAME},
    run_log_line,
};
use anyhow::{Context, Result};
use chrono::{NaiveDateTime, Utc};
use jeeves_state::{
    responsibility_runs as runs,
    schedule_completions::{ScheduleCompletion, ScheduleCompletions},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Config {
    time_zone: String,
    notify: bool,
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "morningBriefing",
    label: "Configured morning briefing",
    uses_model: false,
    internal: true,
    schema: || json!({"type":"object"}),
    describe: |value| {
        let config: Config = serde_json::from_value(value.clone())?;
        Ok(vec![config.time_zone])
    },
    preflight: |_, _| Ok(()),
    run: |input, value| {
        Box::pin(async move {
            let config: Config = serde_json::from_value(value.clone())?;
            let zone: chrono_tz::Tz = config.time_zone.parse()?;
            let date = input.at.with_timezone(&zone).date_naive();
            let body = morning::proposal(input.server, date, zone).await?;
            Ok(Output {
                body: Some(body),
                tools_used: 3,
            })
        })
    },
    settle: Some(|input, value| Box::pin(deliver(input, value))),
};

async fn deliver(
    input: Settlement<'_>,
    value: &serde_json::Value,
) -> Result<(&'static str, String)> {
    let Config { notify, .. } = serde_json::from_value(value.clone())?;
    let server = input.server;
    let wake_key = &input.work.wake_key;
    let minute = wake_key
        .strip_prefix("schedule:daily-morning-briefing:")
        .context("Invalid morning slot")?;
    let at: NaiveDateTime = input
        .work
        .event_json
        .as_deref()
        .context("Missing morning display time")?
        .parse()?;
    let body = input
        .output
        .body
        .as_deref()
        .context("Missing morning draft")?;
    let home = server.settings().home;
    let started = std::time::Instant::now();
    let outcome = {
        runs::proposal(&server.pool, wake_key, body).await?;
        if !runs::admit_send(&server.pool, wake_key, Utc::now().timestamp_millis()).await? {
            Err("Briefing paused before delivery".into())
        } else {
            let file_name = format!("schedule-{NAME}-{}.md", at.format("%Y%m%d-%H%M"));
            let outcome = match crate::schedule_output::write_output(&home, &file_name, body) {
                Ok(())
                    if notify
                        && !runs::current_send(
                            &server.pool,
                            wake_key,
                            Utc::now().timestamp_millis(),
                        )
                        .await? =>
                {
                    Err("Briefing changed before notification; draft saved locally".into())
                }
                Ok(()) if notify => match server.notifier() {
                    Some(notifier) => notifier.send_once(body.to_owned()).await,
                    None => {
                        Err("selected channel is not configured; the proposal is in outbox".into())
                    }
                },
                Ok(()) => Ok(()),
                Err(error) => Err(format!("local proposal delivery failed: {error}")),
            };
            outcome
        }
    };
    let (state, report) = match &outcome {
        Ok(()) => ("produced", ""),
        Err(report) => ("needs_attention", report.as_str()),
    };
    let mut tx = server.pool.begin().await?;
    ScheduleCompletions::settle_on(
        &mut tx,
        ScheduleCompletion {
            name: NAME,
            minute,
            status: state,
            detail: report,
            output: body,
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
        } else if notify {
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
            NAME,
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
    Ok((
        if notify { "confirmed" } else { "draft" },
        "Morning briefing saved".into(),
    ))
}
