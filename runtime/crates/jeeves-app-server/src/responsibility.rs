//! Human RPCs declare a duty. The resident clock records due wakes without acquiring tool grants.
use crate::{dispatch::internal, Server};
use chrono::{DateTime, Local, Utc};
use jeeves_protocol::{
    codes, BudgetView, ErrorObject, ResponsibilityContract, ResponsibilityListResponse,
    ResponsibilitySetPausedParams, ResponsibilityStatus, ResponsibilityView,
};
use jeeves_state::responsibilities::{Responsibilities, ResponsibilityRow};

#[cfg(test)]
mod tests;

fn invalid(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

fn timestamp(ms: i64) -> Result<String, ErrorObject> {
    DateTime::<Utc>::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .ok_or_else(|| internal("invalid stored responsibility timestamp"))
}

fn view(row: ResponsibilityRow) -> Result<ResponsibilityView, ErrorObject> {
    let status = match row.status.as_str() {
        "active" => ResponsibilityStatus::Active,
        "paused" => ResponsibilityStatus::Paused,
        "expired" => ResponsibilityStatus::Expired,
        _ => return Err(internal("invalid stored responsibility status")),
    };
    Ok(ResponsibilityView {
        status,
        last_due_at: row
            .last_slot
            .map(|s| {
                s.checked_mul(60_000)
                    .ok_or_else(|| internal("invalid stored responsibility slot"))
                    .and_then(timestamp)
            })
            .transpose()?,
        contract: contract(row)?,
    })
}

fn contract(row: ResponsibilityRow) -> Result<ResponsibilityContract, ErrorObject> {
    Ok(ResponsibilityContract {
        id: row.id,
        purpose: row.purpose,
        sources: serde_json::from_str(&row.sources_json).map_err(internal)?,
        cron: row.cron,
        expires_at: timestamp(row.expires_ms)?,
        weekly_report: row
            .weekly_report_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(internal)?,
        budget: serde_json::from_str(&row.budget_json).map_err(internal)?,
    })
}

fn validated_row(
    mut p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityRow, ErrorObject> {
    let id = p.id.trim();
    jeeves_agent::schedule::validate_name(id).map_err(invalid)?;
    let purpose = p.purpose.trim();
    if purpose.is_empty() {
        return Err(invalid("a responsibility needs a purpose"));
    }
    let sources: Vec<&str> = p.sources.iter().map(|s| s.trim()).collect();
    if sources.is_empty() || sources.contains(&"") {
        return Err(invalid("each responsibility needs named sources"));
    }
    let cron = p.cron.trim();
    if !jeeves_agent::cron::valid(cron) {
        return Err(invalid("invalid five-field responsibility cron"));
    }
    let expires_ms = DateTime::parse_from_rfc3339(&p.expires_at)
        .map_err(invalid)?
        .timestamp_millis();
    if expires_ms <= now.timestamp_millis() {
        return Err(invalid("the expiry must be in the future"));
    }
    let BudgetView {
        wall_ms,
        tokens,
        tools,
    } = &p.budget;
    if wall_ms.is_none() && tokens.is_none() && tools.is_none() {
        return Err(invalid("declare at least one responsibility budget limit"));
    }
    if let Some(report) = &mut p.weekly_report {
        report.task_id = report.task_id.trim().to_owned();
        report.recipient = report.recipient.trim().to_owned();
        if report.task_id.is_empty() || report.recipient.is_empty() {
            return Err(invalid(
                "weeklyReport needs a research taskId and a Feishu recipient",
            ));
        }
        if sources.len() > 20
            || sources.iter().any(|source| {
                source.len() > 2048
                    || reqwest::Url::parse(source).map_or(true, |url| {
                        !matches!(url.scheme(), "http" | "https")
                            || url.host_str().is_none()
                            || !url.username().is_empty()
                            || url.password().is_some()
                    })
            })
        {
            return Err(invalid("weekly report sources must be at most 20 HTTP(S) evidence URLs without credentials"));
        }
    }
    Ok(ResponsibilityRow {
        id: id.into(),
        purpose: purpose.into(),
        sources_json: serde_json::to_string(&sources).map_err(internal)?,
        cron: cron.into(),
        expires_ms,
        weekly_report_json: p
            .weekly_report
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(internal)?,
        budget_json: crate::task::normalise_budget(Some(&p.budget))?,
        status: "active".into(),
        last_slot: None,
    })
}

pub(crate) fn prepare(
    p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityContract, ErrorObject> {
    contract(validated_row(p, now)?)
}

pub(crate) async fn register(
    server: &Server,
    p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityView, ErrorObject> {
    let row = validated_row(p, now)?;
    if !Responsibilities(&server.pool)
        .register(&row, now.timestamp_millis())
        .await
        .map_err(internal)?
    {
        return Err(invalid("responsibility ID already registered"));
    }
    view(row)
}

fn stop_notice(server: &Server, id: &str, status: &str) {
    eprintln!("(jeeves) responsibility {id} {status}; future wakes stopped");
    if let Some(notifier) = server.notifier() {
        notifier.notify(jeeves_tools::notify::NotifyEvent::ScheduleRan {
            name: format!("responsibility:{id}"),
            status: status.into(),
            preview: "Future wakes stopped".into(),
        });
    }
}

async fn expire(server: &Server, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    for id in Responsibilities(&server.pool)
        .expire(now.timestamp_millis())
        .await?
    {
        stop_notice(server, &id, "expired");
    }
    Ok(())
}

pub(crate) async fn list(
    server: &Server,
    now: DateTime<Utc>,
) -> Result<ResponsibilityListResponse, ErrorObject> {
    expire(server, now).await.map_err(internal)?;
    Ok(ResponsibilityListResponse {
        responsibilities: Responsibilities(&server.pool)
            .list()
            .await
            .map_err(internal)?
            .into_iter()
            .map(view)
            .collect::<Result<_, _>>()?,
    })
}

pub(crate) async fn set_paused(
    server: &Server,
    p: ResponsibilitySetPausedParams,
    now: DateTime<Utc>,
) -> Result<ResponsibilityView, ErrorObject> {
    expire(server, now).await.map_err(internal)?;
    let (row, changed) = Responsibilities(&server.pool)
        .set_paused(&p.id, p.paused, now.timestamp_millis())
        .await
        .map_err(internal)?
        .ok_or_else(|| invalid("unknown responsibility"))?;
    if row.status == "expired" {
        return Err(invalid("an expired responsibility cannot be changed"));
    }
    if p.paused && changed {
        stop_notice(server, &p.id, "paused")
    }
    view(row)
}

pub(crate) async fn tick(server: &Server, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    expire(server, now).await?;
    let rows = Responsibilities(&server.pool);
    let local = now.with_timezone(&Local).naive_local();
    for (id, cron) in rows.candidates(now.timestamp_millis()).await? {
        if jeeves_agent::cron::matches(&cron, local)
            && rows.claim(&id, now.timestamp_millis()).await?
        {
            eprintln!("(jeeves) responsibility {id} due at {now}; wake recorded");
        }
    }
    Ok(())
}
