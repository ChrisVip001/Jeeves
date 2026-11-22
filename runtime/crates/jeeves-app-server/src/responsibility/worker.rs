//! Consume scheduled research wakes without granting a model access to a connector.
use super::{action, feishu};
use crate::{narrow, EventSink, Server};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use jeeves_protocol::{
    BudgetView, ResponsibilityDelivery, ResponsibilityPlan, ResponsibilityReportsResponse,
    WeeklyReportView,
};
use jeeves_state::responsibility_runs::{self as store, Work};
use std::time::{Duration, Instant};

pub(crate) async fn reports(server: &Server, id: &str) -> Result<ResponsibilityReportsResponse> {
    let rows = store::list(&server.pool, id).await?;
    let pending = server.approvals.lock().expect("approvals lock");
    let reports = rows
        .into_iter()
        .map(|r| {
            let approval = pending.get(&r.wake_key).and_then(|waiting| {
                waiting.values().find_map(|entry| {
                    let remaining = entry.remaining();
                    if remaining.is_zero() {
                        return None;
                    }
                    let mut request = entry.request.clone();
                    request.expires_in_ms = narrow(remaining.as_millis() as i64);
                    Some(request)
                })
            });
            WeeklyReportView {
                wake_key: r.wake_key,
                status: r.status,
                body: r.body,
                detail: r.detail,
                tools_used: r.tools_used,
                wall_ms: r.wall_ms,
                approval,
            }
        })
        .collect();
    Ok(ResponsibilityReportsResponse { reports })
}

pub(crate) async fn run(
    server: Server,
    sink: EventSink,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    while !*stop.borrow() {
        match store::take(&server.pool, Utc::now().timestamp_millis()).await {
            Ok(Some(work)) => {
                if let Err(error) = process(&server, &sink, &work).await {
                    eprintln!(
                        "(jeeves) responsibility {} bookkeeping failed: {error}",
                        work.wake_key
                    );
                }
                continue;
            }
            Err(error) => eprintln!("(jeeves) responsibility inbox unavailable: {error}"),
            Ok(None) => {}
        }
        tokio::select! {
            _ = stop.changed() => break,
            () = tokio::time::sleep(Duration::from_secs(2)) => {}
        }
    }
    Ok(())
}

pub(super) async fn process(server: &Server, sink: &EventSink, work: &Work) -> Result<()> {
    let started = Instant::now();
    let budget: BudgetView = serde_json::from_str(&work.budget_json)?;
    let remaining = (work.expires_ms - Utc::now().timestamp_millis()).max(0) as u64;
    let max_wall = budget
        .wall_ms
        .map_or(remaining, |limit| remaining.min(limit as u64));
    let execution = tokio::time::timeout(
        Duration::from_millis(max_wall),
        execute(
            server,
            sink,
            work,
            &budget,
            started + Duration::from_millis(max_wall),
        ),
    );
    tokio::pin!(execution);
    let result = loop {
        tokio::select! {
            result = &mut execution => break result,
            () = tokio::time::sleep(Duration::from_secs(10)) => {
                if !store::heartbeat(&server.pool, &work.wake_key, Utc::now().timestamp_millis()).await? {
                    break Ok(Err(anyhow::anyhow!("Execution lease lost, responsibility changed, or no progress; review required")));
                }
            }
        }
    };
    let (status, detail) = match result {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(error)) => ("review", error.to_string()),
        Err(_) => (
            "review",
            "Wall budget or responsibility deadline reached; check delivery before retrying".into(),
        ),
    };
    let waiting = server
        .approvals
        .lock()
        .expect("approvals lock")
        .remove(&work.wake_key);
    if let Some(waiting) = waiting {
        for id in waiting.keys() {
            server.grants.pending_remove(&work.wake_key, id).await?;
        }
    }
    store::finish(
        &server.pool,
        &work.wake_key,
        status,
        &detail,
        narrow(started.elapsed().as_millis() as i64),
    )
    .await?;
    eprintln!(
        "(jeeves) responsibility {}: {status} — {detail}",
        work.wake_key
    );
    Ok(())
}

async fn execute(
    server: &Server,
    sink: &EventSink,
    work: &Work,
    budget: &BudgetView,
    deadline: Instant,
) -> Result<(&'static str, String)> {
    if let Some(json) = &work.plan_json {
        let plan = serde_json::from_str(json)?;
        return execute_plan(server, sink, work, &plan, budget, deadline).await;
    }
    if let Some(options) = &work.digest_json {
        let instant = DateTime::from_timestamp(
            work.slot
                .checked_mul(60)
                .context("Invalid scheduled slot")?,
            0,
        )
        .context("Invalid scheduled time")?;
        let sources: Vec<String> = serde_json::from_str(&work.sources_json)?;
        let body = crate::responsibility::digest::render(
            server,
            &serde_json::from_str(options)?,
            &sources,
            instant,
        )
        .await?;
        ensure!(Instant::now() < deadline, "Digest deadline reached");
        ensure!(
            store::deliver_chat(
                &server.pool,
                &work.wake_key,
                &body,
                sources.len() as i32,
                Utc::now().timestamp_millis()
            )
            .await?,
            "Digest was stopped or revised before delivery"
        );
        return Ok(("confirmed", "Delivered in main chat".into()));
    }
    anyhow::bail!("Responsibility has no executable action")
}

async fn execute_plan(
    server: &Server,
    sink: &EventSink,
    work: &Work,
    plan: &ResponsibilityPlan,
    budget: &BudgetView,
    deadline: Instant,
) -> Result<(&'static str, String)> {
    let preflight = action::validate(plan, budget).and_then(|()| {
        action::preflight(server, plan)?;
        action::find(&plan.action)
    });
    let adapter = match preflight {
        Ok(adapter) => adapter,
        Err(error) => {
            store::outcome(
                &server.pool,
                &work.wake_key,
                "blocked",
                &error.to_string(),
                None,
                0,
            )
            .await?;
            return Ok(("blocked", error.to_string()));
        }
    };
    let at = match work.due_ms {
        Some(due) => {
            DateTime::from_timestamp_millis(due).context("Invalid occurrence timestamp")?
        }
        None => work.created_at.parse()?,
    };
    let result = (adapter.run)(
        action::Input {
            server,
            at,
            wake_key: &work.wake_key,
            event: work.event_json.as_deref(),
            budget,
        },
        &plan.action.config,
    )
    .await?;
    ensure!(Instant::now() < deadline, "Responsibility deadline reached");
    if let (Some(body), ResponsibilityDelivery::Feishu { recipient }) =
        (&result.body, &plan.delivery)
    {
        return feishu::deliver(
            server,
            sink,
            &work.wake_key,
            recipient,
            body,
            budget,
            deadline,
        )
        .await;
    }
    if let Some(body) = result
        .body
        .as_deref()
        .filter(|_| matches!(plan.delivery, ResponsibilityDelivery::Chat))
    {
        ensure!(
            store::deliver_chat(
                &server.pool,
                &work.wake_key,
                body,
                result.tools_used,
                Utc::now().timestamp_millis()
            )
            .await?,
            "Responsibility was stopped or revised before delivery"
        );
        return Ok(("confirmed", "Delivered in main chat".into()));
    }
    let (status, detail) = if result.body.is_some() {
        ("draft", "Private draft saved")
    } else {
        ("skipped", "No matching records")
    };
    store::outcome(
        &server.pool,
        &work.wake_key,
        status,
        detail,
        result.body.as_deref(),
        result.tools_used,
    )
    .await?;
    Ok((status, detail.into()))
}
