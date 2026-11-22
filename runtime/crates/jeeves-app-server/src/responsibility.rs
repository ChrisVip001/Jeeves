//! Human RPCs declare a duty. The resident clock records due wakes without acquiring tool grants.
use crate::{dispatch::internal, Server};
use chrono::{DateTime, Local, Utc};
use jeeves_protocol::{
    codes, BudgetView, ErrorObject, ResponsibilityContract, ResponsibilityListResponse,
    ResponsibilitySetPausedParams, ResponsibilityStatus, ResponsibilityView,
};
use jeeves_state::responsibilities::{Responsibilities, ResponsibilityRow};

pub(crate) mod action;
pub(crate) mod digest;
pub(crate) mod events;
mod feishu;
mod model;
mod morning;
#[cfg(test)]
mod plans_tests;
pub(crate) mod price;
mod records;
mod research;
pub(crate) mod reviews;
#[cfg(test)]
mod tests;
pub(crate) mod trigger;
pub(crate) mod worker;

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
    use jeeves_protocol::ResponsibilityClock;
    let clock = match row.clock_owner.as_str() {
        "cron" => ResponsibilityClock::Cron,
        "morning" => ResponsibilityClock::Morning,
        "price-watch" => ResponsibilityClock::PriceWatch,
        _ => return Err(internal("invalid stored responsibility clock")),
    };
    Ok(ResponsibilityView {
        clock,
        revision: row.revision,
        status,
        last_due_at: row
            .last_slot
            .map(|s| {
                s.checked_mul(60_000)
                    .ok_or_else(|| internal("invalid stored responsibility slot"))
                    .and_then(timestamp)
            })
            .transpose()?,
        presentation: if row.research_task_id.is_some() || row.clock_owner != "cron" {
            None
        } else {
            plan(&row)?.as_ref().map(action::presentation)
        },
        contract: contract(row)?,
    })
}

fn plan(
    row: &ResponsibilityRow,
) -> Result<Option<jeeves_protocol::ResponsibilityPlan>, ErrorObject> {
    row.plan_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(internal)
}

fn contract(row: ResponsibilityRow) -> Result<ResponsibilityContract, ErrorObject> {
    Ok(ResponsibilityContract {
        plan: if row.research_task_id.is_some() || row.clock_owner != "cron" {
            None
        } else {
            plan(&row)?
        },
        id: row.id,
        purpose: row.purpose,
        digest: row
            .digest_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(internal)?,
        sources: serde_json::from_str(&row.sources_json).map_err(internal)?,
        cron: row.cron,
        expires_at: timestamp(row.expires_ms)?,
        weekly_report: row
            .research_task_id
            .zip(row.feishu_recipient)
            .map(|(task_id, recipient)| jeeves_protocol::FeishuWeeklyReport { task_id, recipient }),
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
    if (p.plan.is_none() && sources.is_empty()) || sources.contains(&"") {
        return Err(invalid("each responsibility needs named sources"));
    }
    let cron = p.cron.trim();
    if p.plan.is_none() && !jeeves_agent::cron::valid(cron) {
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
    if let Some(options) = &p.digest {
        options
            .time_zone
            .parse::<chrono_tz::Tz>()
            .map_err(invalid)?;
        if p.weekly_report.is_some()
            || sources
                .iter()
                .any(|s| !matches!(*s, "calendar" | "openTasks" | "unreadMail"))
            || sources
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != sources.len()
        {
            return Err(invalid(
                "A chat digest needs distinct calendar, openTasks or unreadMail sources",
            ));
        }
        if tools.is_some_and(|n| n < sources.len() as i32) {
            return Err(invalid("The tool budget must cover the selected sources"));
        }
    }
    if let Some(report) = &mut p.weekly_report {
        report.task_id = report.task_id.trim().to_owned();
        report.recipient = report.recipient.trim().to_owned();
        if report.task_id.is_empty() || report.recipient.is_empty() {
            return Err(invalid(
                "weeklyReport needs a research taskId and a Feishu recipient",
            ));
        }
        validate_research_sources(sources.iter().copied()).map_err(invalid)?;
    }

    if let Some(plan) = &p.plan {
        if p.digest.is_some() || p.weekly_report.is_some() {
            return Err(invalid("Use either a plan or the legacy definition"));
        }
        action::allow_proposal(plan).map_err(invalid)?;
        action::validate(plan, &p.budget).map_err(invalid)?;
        trigger::validate(&plan.trigger, now, expires_ms).map_err(invalid)?;
    }
    Ok(ResponsibilityRow {
        plan_json: p
            .plan
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(internal)?,
        next_due_ms: None,
        clock_owner: "cron".into(),
        revision: 1,
        digest_json: p
            .digest
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(internal)?,
        id: id.into(),
        purpose: purpose.into(),
        sources_json: serde_json::to_string(&sources).map_err(internal)?,
        cron: cron.into(),
        expires_ms,
        research_task_id: p.weekly_report.as_ref().map(|r| r.task_id.clone()),
        feishu_recipient: p.weekly_report.map(|r| r.recipient),
        budget_json: crate::task::normalise_budget(Some(&p.budget))?,
        status: "active".into(),
        last_slot: None,
    })
}

fn validate_research_sources<'a>(sources: impl Iterator<Item = &'a str>) -> anyhow::Result<()> {
    let sources: Vec<_> = sources.collect();
    anyhow::ensure!(
        !sources.is_empty()
            && sources.len() <= 20
            && sources.iter().all(|source| {
                source.len() <= 2048
                    && reqwest::Url::parse(source).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https")
                            && url.host_str().is_some()
                            && url.username().is_empty()
                            && url.password().is_none()
                    })
            }),
        "Select up to 20 HTTP(S) evidence URLs without credentials"
    );
    Ok(())
}

fn activate(
    mut contract: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityRow, ErrorObject> {
    if let Some(plan) = &mut contract.plan {
        trigger::anchor(&mut plan.trigger, now).map_err(invalid)?;
    }
    let mut row = validated_row(contract, now)?;
    if let Some(plan) = plan(&row)? {
        row.next_due_ms = trigger::first(&plan.trigger, now).map_err(invalid)?;
    } else if let (Some(task), Some(recipient)) = (&row.research_task_id, &row.feishu_recipient) {
        row.plan_json = Some(serde_json::json!({
            "trigger":{"kind":"cron","expression":row.cron,"timeZone":"UTC"},
            "action":{"kind":"researchDigest","version":1,"config":{"taskId":task,"sources":serde_json::from_str::<serde_json::Value>(&row.sources_json).map_err(internal)?}},
            "delivery":{"kind":"feishu","recipient":recipient}
        }).to_string());
    }
    Ok(row)
}

pub(crate) fn prepare(
    p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityContract, ErrorObject> {
    contract(validated_row(p, now)?)
}

pub(crate) async fn sample(
    server: &Server,
    p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<String, ErrorObject> {
    let checked = prepare(p, now)?;
    if let Some(plan) = &checked.plan {
        return action::sample(server, plan, now).await.map_err(invalid);
    }
    let options = checked
        .digest
        .as_ref()
        .ok_or_else(|| invalid("Select a digest to preview its sources"))?;
    let deadline = DateTime::parse_from_rfc3339(&checked.expires_at)
        .map_err(invalid)?
        .timestamp_millis()
        - now.timestamp_millis();
    let limit = checked
        .budget
        .wall_ms
        .map(i64::from)
        .unwrap_or(deadline)
        .min(deadline);
    tokio::time::timeout(
        std::time::Duration::from_millis(limit as u64),
        digest::render(server, options, &checked.sources, now),
    )
    .await
    .map_err(|_| invalid("Digest preview deadline reached"))?
    .map_err(invalid)
}

pub(crate) async fn register(
    server: &Server,
    p: ResponsibilityContract,
    now: DateTime<Utc>,
) -> Result<ResponsibilityView, ErrorObject> {
    let row = activate(p, now)?;
    if !Responsibilities(&server.pool)
        .register(&row, now.timestamp_millis())
        .await
        .map_err(internal)?
    {
        return Err(invalid("responsibility ID already registered"));
    }
    view(row)
}

pub(crate) async fn amend(
    server: &Server,
    p: jeeves_protocol::ResponsibilityAmendParams,
    now: DateTime<Utc>,
) -> Result<ResponsibilityView, ErrorObject> {
    let row = activate(p.contract, now)?;
    let changed = Responsibilities(&server.pool)
        .amend(&row, p.expected_revision, now.timestamp_millis())
        .await
        .map_err(internal)?
        .ok_or_else(|| invalid("Responsibility changed or expired. Review its latest version."))?;
    view(changed)
}

async fn stop_notice(server: &Server, id: &str, status: &str) {
    eprintln!("(jeeves) responsibility {id} {status}; future wakes stopped");
    if let Some(notifier) = server.notifier() {
        match sqlx::query_scalar::<_, bool>(
            "SELECT digest_json IS NULL AND plan_json IS NULL AND clock_owner='cron' FROM responsibilities WHERE id=?",
        )
        .bind(id)
        .fetch_one(&server.pool)
        .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(error) => {
                eprintln!("(jeeves) could not check responsibility {id} notification destination: {error}");
                return;
            }
        }
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
        stop_notice(server, &id, "expired").await;
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
        stop_notice(server, &p.id, "paused").await
    }
    view(row)
}

pub(crate) async fn tick(server: &Server, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    expire(server, now).await?;
    trigger::tick(server, now).await?;
    let rows = Responsibilities(&server.pool);
    for (id, cron, revision, zone) in rows.candidates(now.timestamp_millis()).await? {
        let local = match zone {
            Some(zone) => now
                .with_timezone(
                    &zone
                        .parse::<chrono_tz::Tz>()
                        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?,
                )
                .naive_local(),
            None => now.with_timezone(&Local).naive_local(),
        };
        if jeeves_agent::cron::matches(&cron, local)
            && rows.claim(&id, revision, now.timestamp_millis()).await?
        {
            eprintln!("(jeeves) responsibility {id} due at {now}; wake recorded");
        }
    }
    Ok(())
}
