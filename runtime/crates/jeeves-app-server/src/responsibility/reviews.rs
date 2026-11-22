//! Owner-only proposals. A human decision consumes exactly the reviewed fields and revision.
use super::*;
use jeeves_protocol::{ResponsibilityDecision, ResponsibilityReview};
use jeeves_tools::Tool;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    action: String,
    id: Option<String>,
    fields: Option<Value>,
    #[serde(rename = "expectedRevision")]
    expected_revision: Option<i32>,
}

pub(crate) fn tool(server: Server, turn: String, message: String) -> Tool {
    Tool {
        name:"recurring_work".into(),
        description:format!("Propose an owner's recurring responsibility. Local time {}. Always list before choosing an existing id and expectedRevision. If several match, ask. Create or amend leaves a review for the owner. Describe the pending change. Never claim activation, pausing or delivery before human confirmation. The owner reviews the card. Keep the reply brief. Paragraph layout renders exactly one paragraph per selected source, including a single paragraph when one source remains. Describe only fields on the card. Use an explicit time zone and tell the person any assumed time or expiry.",Local::now().to_rfc3339()),
        input_schema:json!({"type":"object","required":["action"],"additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","capabilities","create","amend","pause","resume"]},"id":{"type":"string","description":"Stable lowercase ASCII slug; list existing duties to find an id."},"expectedRevision":{"type":"integer"},
            "fields":{"type":"object","description":"On amendment supply changed top-level fields only. For a weekly Feishu research report use existing research taskId, permitted evidence URLs and recipient instead of digest.","additionalProperties":false,"properties":{
                "plan":{"type":"object","description":"Trigger, action and delivery. Call capabilities for supported kinds and event sources."},"purpose":{"type":"string"},"sources":{"type":"array","items":{"type":"string"},"description":"Digest: calendar, openTasks or unreadMail."},"cron":{"type":"string","description":"Five-field schedule."},"expiresAt":{"type":"string","description":"Optional RFC3339 end; defaults to one year for a new duty."},
                "digest":{"type":"object","required":["timeZone","calendarWindow","layout"],"properties":{"timeZone":{"type":"string"},"calendarWindow":{"enum":["today","tomorrow","thisWeek","nextWeek"]},"layout":{"enum":["paragraphs","list"]}}},
                "weeklyReport":{"type":"object","required":["taskId","recipient"],"properties":{"taskId":{"type":"string"},"recipient":{"type":"string"}}}
            }}
        }}),
        handler:Arc::new(move |_ctx,args|{
            let (server,turn,message)=(server.clone(),turn.clone(),message.clone());
            Box::pin(async move {
                let result=propose(&server,&turn,&message,serde_json::from_value(args)?).await.map_err(|e|anyhow::anyhow!(e.message))?;
                Ok(serde_json::to_string(&result)?)
            })
        }),
    }
}

async fn propose(
    server: &Server,
    turn: &str,
    message: &str,
    c: Change,
) -> Result<Value, ErrorObject> {
    if c.action == "capabilities" {
        return Ok(catalog(server));
    }
    let now = Utc::now();
    let duties = super::list(server, now).await?.responsibilities;
    if c.action == "list" {
        return serde_json::to_value(duties).map_err(internal);
    }
    let id = c.id.as_deref().ok_or_else(|| invalid("Choose a duty id"))?;
    let existing = duties.into_iter().find(|r| r.contract.id == id);
    let (mut body, revision) = match (c.action.as_str(), existing) {
        ("create", None) => (
            json!({"id":id,"budget":{"tools":3,"wallMs":90000,"tokens":4096},"expiresAt":(now+chrono::Duration::days(365)).to_rfc3339()}),
            0,
        ),
        ("amend", Some(row)) if row.clock != jeeves_protocol::ResponsibilityClock::Cron => {
            return Err(invalid(match row.clock {
                jeeves_protocol::ResponsibilityClock::PriceWatch => "Select the original price watch in My Work and state the new limit.",
                _ => "The configured morning briefing uses its original clock. Offer a separate private digest for a different time or source selection.",
            }));
        }
        ("amend" | "pause" | "resume", Some(row)) if c.expected_revision == Some(row.revision) => (
            serde_json::to_value(row.contract).map_err(internal)?,
            row.revision,
        ),
        _ => {
            return Err(invalid(
                "Duty or revision changed. List the current responsibilities.",
            ))
        }
    };
    if let Some(fields) = c.fields {
        let fields = fields
            .as_object()
            .ok_or_else(|| invalid("Expected changed fields"))?;
        if !matches!(c.action.as_str(), "create" | "amend")
            || fields.keys().any(|k| {
                !matches!(
                    k.as_str(),
                    "purpose"
                        | "sources"
                        | "cron"
                        | "expiresAt"
                        | "weeklyReport"
                        | "digest"
                        | "plan"
                )
            })
        {
            return Err(invalid("Invalid fields for responsibility change"));
        }
        for (k, v) in fields {
            body[k] = v.clone();
        }
    }
    let contract = prepare(serde_json::from_value(body).map_err(invalid)?, now)?;
    if matches!(c.action.as_str(), "create" | "amend")
        && contract.plan.is_none()
        && contract.digest.is_none()
        && contract.weekly_report.is_none()
    {
        return Err(invalid("Select a supported recurring output"));
    }
    let review = ResponsibilityReview {
        id: turn.into(),
        action: c.action,
        request: message.into(),
        expected_revision: revision,
        presentation: contract.plan.as_ref().map(|plan| preview(server, plan)),
        contract,
    };
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let saved=sqlx::query("INSERT INTO responsibility_reviews(id,duty_id,expected_revision,action,request,contract_json,created_ms,state) VALUES(?,?,?,?,?,?,?,'pending') ON CONFLICT(id) DO NOTHING")
        .bind(turn).bind(id).bind(revision).bind(&review.action).bind(message).bind(serde_json::to_string(&review.contract).map_err(internal)?).bind(now.timestamp_millis()).execute(&mut *tx).await.map_err(internal)?;
    if saved.rows_affected() != 1 {
        return Err(invalid("This message already has a review card"));
    }
    sqlx::query("UPDATE responsibility_reviews SET state='superseded' WHERE duty_id=? AND state='pending' AND id<>?").bind(id).bind(turn).execute(&mut *tx).await.map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    serde_json::to_value(review).map_err(internal)
}

pub(crate) async fn list(server: &Server) -> Result<Vec<ResponsibilityReview>, ErrorObject> {
    let rows:Vec<(String,String,String,i32,String)>=sqlx::query_as("SELECT id,action,request,expected_revision,contract_json FROM responsibility_reviews WHERE state='pending' ORDER BY rowid DESC").fetch_all(&server.pool).await.map_err(internal)?;
    rows.into_iter()
        .map(|(id, action, request, expected_revision, json)| {
            let contract: ResponsibilityContract = serde_json::from_str(&json).map_err(internal)?;
            Ok(ResponsibilityReview {
                id,
                action,
                request,
                expected_revision,
                presentation: contract.plan.as_ref().map(|plan| preview(server, plan)),
                contract,
            })
        })
        .collect()
}

pub(crate) async fn decide(
    server: &Server,
    decision: ResponsibilityDecision,
) -> Result<bool, ErrorObject> {
    let now = Utc::now();
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let state = if decision.accept {
        "accepted"
    } else {
        "dismissed"
    };
    let record:Option<(String,String,i32,String)>=sqlx::query_as("UPDATE responsibility_reviews SET state=? WHERE id=? AND state='pending' RETURNING duty_id,action,expected_revision,contract_json")
        .bind(state).bind(decision.id).fetch_optional(&mut *tx).await.map_err(internal)?;
    let Some((id, action, revision, json)) = record else {
        return Err(invalid("Review already closed or superseded"));
    };
    if decision.accept {
        let contract: ResponsibilityContract = serde_json::from_str(&json).map_err(internal)?;
        if action != "pause" {
            if let Some(plan) = &contract.plan {
                action::preflight(server, plan).map_err(invalid)?;
            }
        }
        let row = if matches!(action.as_str(), "create" | "amend") {
            activate(contract, now)?
        } else {
            validated_row(contract, now)?
        };
        let changed = match action.as_str() {
            "create" => Responsibilities::register_on(&mut tx, &row, now.timestamp_millis())
                .await
                .map_err(internal)?,
            "amend" => Responsibilities::amend_on(&mut tx, &row, revision, now.timestamp_millis())
                .await
                .map_err(internal)?
                .is_some(),
            "pause" | "resume" => Responsibilities::set_paused_on(
                &mut tx,
                &id,
                action == "pause",
                Some(revision),
                now.timestamp_millis(),
            )
            .await
            .map_err(internal)?
            .is_some_and(|(_, changed)| changed),
            _ => return Err(internal("Invalid review action")),
        };
        if !changed {
            return Err(invalid(
                "Responsibility changed. Review the latest definition.",
            ));
        }
    }
    tx.commit().await.map_err(internal)?;
    if decision.accept && action == "pause" {
        stop_notice(server, &id, "paused").await;
    }
    Ok(true)
}

fn catalog(server: &Server) -> Value {
    json!({
        "triggers":[
            {"kind":"at","at":"ISO date-time with offset"},
            {"kind":"after","delaySeconds":"positive integer"},
            {"kind":"every","intervalSeconds":"integer >= 60"},
            {"kind":"cron","expression":"five fields","timeZone":"IANA zone"},
            {"kind":"event","source":"approved source","eventType":"approved event type"}
        ],
        "actions":action::catalog(),
        "eventSources":super::events::catalog(server),
        "deliveries":[{"kind":"chat"},{"kind":"draft"}]
    })
}

fn preview(
    server: &Server,
    plan: &jeeves_protocol::ResponsibilityPlan,
) -> jeeves_protocol::ResponsibilityPresentation {
    let mut card = action::presentation(plan);
    if card.blocked.is_none() {
        card.blocked = action::preflight(server, plan)
            .err()
            .map(|error| error.to_string());
    }
    card
}
