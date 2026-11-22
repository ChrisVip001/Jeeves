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
        description:format!("Propose an owner's recurring responsibility. Local time {}. Before creating a plan or replacing a plan element, call capabilities. Use plan for the complete trigger, action and delivery. For an old definition compose a complete plan from the capabilities. Compare the exact field values shown by the source, including media types and units. Static action messages have no placeholder interpolation. Always list before choosing an existing id and expectedRevision. If several match, ask. Create or amend leaves a review for the owner. Describe the pending change. Never claim activation, pausing or delivery before human confirmation. The owner reviews the card. Keep the reply brief. Paragraph layout renders exactly one paragraph per selected source, including a single paragraph when one source remains. Describe only fields on the card. Use an explicit time zone and tell the person any assumed time or expiry.",Local::now().to_rfc3339()),
        input_schema:json!({"type":"object","required":["action"],"additionalProperties":false,"properties":{
            "action":{"type":"string","enum":["list","capabilities","create","amend","pause","resume"]},"id":{"type":"string","description":"Stable lowercase ASCII slug; list existing duties to find an id."},"expectedRevision":{"type":"integer"},
            "fields":{"type":"object","description":"On amendment supply changed top-level fields only. Use the capabilities catalog to select the action and delivery.","additionalProperties":false,"properties":{
                "plan":{"type":"object","description":"Complete trigger, action and delivery. Call capabilities for the supported kinds and sources."},
                "purpose":{"type":"string"},"expiresAt":{"type":"string","description":"Optional RFC3339 end; defaults to one year for a new duty."}
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
    if matches!(c.action.as_str(), "amend" | "resume")
        && existing
            .as_ref()
            .is_some_and(|row| row.migration_note.is_some())
        && (c.action == "resume"
            || c.fields
                .as_ref()
                .and_then(|fields| fields.get("plan"))
                .is_none_or(Value::is_null))
    {
        return Err(invalid(
            "Review an updated plan before resuming this imported duty",
        ));
    }
    let original_action = existing
        .as_ref()
        .and_then(|row| row.contract.plan.as_ref())
        .map(|plan| plan.action.clone());
    let (mut body, revision) = match (c.action.as_str(), existing) {
        ("create", None) => (
            json!({"id":id,"budget":{"tools":3,"wallMs":90000,"tokens":4096},"expiresAt":(now+chrono::Duration::days(365)).to_rfc3339()}),
            0,
        ),
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
            || fields
                .keys()
                .any(|k| !matches!(k.as_str(), "purpose" | "expiresAt" | "plan"))
        {
            return Err(invalid("Invalid fields for responsibility change"));
        }
        for (k, v) in fields {
            body[k] = v.clone();
        }
    }
    if body.get("plan").is_some_and(|value| !value.is_null())
        && matches!(c.action.as_str(), "create" | "amend")
    {
        body.as_object_mut()
            .expect("contract object")
            .remove("weeklyReport");
        body.as_object_mut()
            .expect("contract object")
            .remove("digest");
    }
    let mut requested: ResponsibilityContract = serde_json::from_value(body).map_err(invalid)?;
    if matches!(c.action.as_str(), "create" | "amend") {
        if let Some(plan) = &mut requested.plan {
            super::model::pin(server, &mut plan.action, original_action.as_ref())
                .map_err(invalid)?;
        }
    }
    let contract = if c.action == "pause" {
        requested
    } else {
        prepare(requested, now)?
    };
    if matches!(c.action.as_str(), "create" | "amend") && contract.plan.is_none() {
        return Err(invalid("Select a supported trigger, action and delivery"));
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
        let changed = match action.as_str() {
            "create" => {
                let row = activate(contract, now)?;
                Responsibilities::register_on(&mut tx, &row, now.timestamp_millis())
                    .await
                    .map_err(internal)?
            }
            "amend" => {
                let row = activate(contract, now)?;
                Responsibilities::amend_on(&mut tx, &row, revision, now.timestamp_millis())
                    .await
                    .map_err(internal)?
                    .is_some()
            }
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
    let mut deliveries = vec![json!({"kind":"chat"}), json!({"kind":"draft"})];
    deliveries.extend(crate::connector_settings::publish::enabled(
        &server.settings().home,
    ));
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
        "skills":super::model::catalog(server),
        "deliveries":deliveries
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
