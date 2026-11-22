//! Accept authenticated source observations into the same wake inbox as timer slots.
use super::{internal, invalid, trigger, ErrorObject, Server};
use chrono::{DateTime, Utc};
use jeeves_protocol::{ResponsibilityEvent, ResponsibilityEventReceipt, ResponsibilityTrigger};
use serde::Deserialize;

#[derive(Deserialize, Default)]
struct Hooks {
    hooks: Vec<Source>,
}

#[derive(Deserialize)]
struct Source {
    name: String,
    event_type: Option<String>,
    signature: Option<bool>,
    enabled: Option<bool>,
    #[serde(default)]
    event_fields: std::collections::BTreeMap<String, Field>,
    #[serde(default)]
    event_field_examples: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) enum Field {
    String,
    Integer,
    Boolean,
    StringArray,
}

fn sources(server: &Server) -> anyhow::Result<Vec<Source>> {
    let file = server.settings().home.join("webhooks.json");
    let hooks: Hooks = match std::fs::read_to_string(file) {
        Ok(json) => serde_json::from_str(&json)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Hooks::default(),
        Err(err) => return Err(err.into()),
    };
    Ok(hooks
        .hooks
        .into_iter()
        .filter(|s| s.enabled != Some(false) && s.signature == Some(true) && s.event_type.is_some())
        .collect())
}

pub(super) fn catalog(server: &Server) -> serde_json::Value {
    match sources(server) {
        Ok(sources) => serde_json::Value::Array(
            sources
                .into_iter()
                .map(|s| serde_json::json!({"source":s.name,"eventType":s.event_type,"fields":s.event_fields,"fieldExamples":s.event_field_examples}))
                .collect(),
        ),
        Err(error) => serde_json::json!({"blocked":error.to_string()}),
    }
}

fn source(server: &Server, source: &str, event_type: &str) -> anyhow::Result<Source> {
    sources(server)?
        .into_iter()
        .find(|entry| entry.name == source && entry.event_type.as_deref() == Some(event_type))
        .ok_or_else(|| {
            anyhow::anyhow!("Connect and enable the signed event source {source} ({event_type})")
        })
}

pub(super) fn preflight(
    server: &Server,
    trigger: &ResponsibilityTrigger,
) -> anyhow::Result<Option<std::collections::BTreeMap<String, Field>>> {
    if let ResponsibilityTrigger::Event {
        source: binding,
        event_type,
    } = trigger
    {
        return Ok(Some(source(server, binding, event_type)?.event_fields));
    }
    Ok(None)
}

pub(super) async fn sample(
    server: &Server,
    trigger: &ResponsibilityTrigger,
) -> anyhow::Result<Option<String>> {
    let ResponsibilityTrigger::Event { source, event_type } = trigger else {
        return Ok(None);
    };
    Ok(latest(server, source, event_type, 86_400)
        .await?
        .map(|(fields, _)| fields))
}

pub(super) async fn latest(
    server: &Server,
    source: &str,
    event_type: &str,
    max_age_seconds: u32,
) -> anyhow::Result<Option<(String, Option<String>)>> {
    Ok(sqlx::query_as("SELECT fields,event_id FROM responsibility_event_samples WHERE source=? AND event_type=? AND sampled_ms>?")
        .bind(source).bind(event_type).bind(Utc::now().timestamp_millis()-i64::from(max_age_seconds)*1000)
        .fetch_optional(&server.pool).await?)
}

fn field_matches(field: Field, v: &serde_json::Value) -> bool {
    match field {
        Field::String => v.as_str().is_some_and(|s| s.len() <= 1024),
        Field::Integer => v.as_i64().is_some(),
        Field::Boolean => v.is_boolean(),
        Field::StringArray => v
            .as_array()
            .is_some_and(|a| a.len() <= 32 && a.iter().all(|v| field_matches(Field::String, v))),
    }
}

pub(crate) async fn admit(
    server: &Server,
    input: ResponsibilityEvent,
    now: DateTime<Utc>,
) -> Result<ResponsibilityEventReceipt, ErrorObject> {
    let source = source(server, &input.source, &input.event_type).map_err(invalid)?;
    let body = serde_json::to_string(&input.fields).map_err(invalid)?;
    if !trigger::slug(&input.source)
        || !trigger::slug(&input.event_type)
        || !trigger::slug(&input.event_id)
        || input.cursor.is_empty()
        || input.cursor.len() > 512
        || input
            .previous_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 512 || cursor == &input.cursor)
        || body.len() > 8192
        || !input.fields.is_object()
    {
        return Err(invalid("Invalid event identity, cursor, or fields"));
    }
    if !source.event_fields.is_empty()
        && input.fields.as_object().is_some_and(|record| {
            record.iter().any(|(name, value)| {
                source
                    .event_fields
                    .get(name)
                    .is_none_or(|&kind| !field_matches(kind, value))
            })
        })
    {
        return Err(invalid("Event fields do not match the enabled source"));
    }
    let ms = now.timestamp_millis();
    let occurred = DateTime::parse_from_rfc3339(&input.occurred_at)
        .map_err(invalid)?
        .timestamp_millis();
    if occurred > ms + 60_000 {
        return Err(invalid("Event timestamp is in the future"));
    }
    let fingerprint: String = ring::digest::digest(
        &ring::digest::SHA256,
        format!("{occurred}:{body}").as_bytes(),
    )
    .as_ref()
    .iter()
    .map(|byte| format!("{byte:02x}"))
    .collect();
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let existing: Option<(String,String)> = sqlx::query_as(
        "SELECT cursor,fingerprint FROM responsibility_event_positions WHERE source=? AND event_type=? AND event_id=?",
    ).bind(&input.source).bind(&input.event_type).bind(&input.event_id).fetch_optional(&mut *tx).await.map_err(internal)?;
    if let Some((cursor, previous)) = existing {
        if previous != fingerprint || cursor != input.cursor {
            return Err(invalid(
                "Event identity reused with different content or cursor",
            ));
        }
        eprintln!("(jeeves) duplicate event on {} skipped", input.source);
        return Ok(ResponsibilityEventReceipt {
            admitted: 0,
            duplicate: true,
        });
    }
    let cursor: Option<String> = sqlx::query_scalar(
        "SELECT cursor FROM responsibility_event_cursors WHERE source=? AND event_type=?",
    )
    .bind(&input.source)
    .bind(&input.event_type)
    .fetch_optional(&mut *tx)
    .await
    .map_err(internal)?;
    if cursor != input.previous_cursor {
        return Err(invalid(
            "Event cursor has changed; replay from the last acknowledged cursor",
        ));
    }
    sqlx::query("INSERT INTO responsibility_event_positions(source,event_type,event_id,cursor,fingerprint) VALUES(?,?,?,?,?)")
        .bind(&input.source).bind(&input.event_type).bind(&input.event_id).bind(&input.cursor).bind(fingerprint)
        .execute(&mut *tx).await.map_err(internal)?;
    sqlx::query(
        "INSERT INTO responsibility_event_cursors(source,event_type,cursor) VALUES(?,?,?)
        ON CONFLICT(source,event_type) DO UPDATE SET cursor=excluded.cursor",
    )
    .bind(&input.source)
    .bind(&input.event_type)
    .bind(&input.cursor)
    .execute(&mut *tx)
    .await
    .map_err(internal)?;
    if ms - occurred <= 900_000 {
        sqlx::query("INSERT INTO responsibility_event_samples(source,event_type,fields,sampled_ms,event_id) VALUES(?,?,?,?,?)
             ON CONFLICT(source,event_type) DO UPDATE SET fields=excluded.fields,sampled_ms=excluded.sampled_ms,event_id=excluded.event_id
             WHERE excluded.sampled_ms>=responsibility_event_samples.sampled_ms")
            .bind(&input.source).bind(&input.event_type).bind(&body).bind(occurred).bind(&input.event_id)
            .execute(&mut *tx).await.map_err(internal)?;
    }
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM wake_inbox WHERE state='pending' AND target_task LIKE 'responsibility:%'")
        .fetch_one(&mut *tx).await.map_err(internal)?;
    let rows: Vec<(String,i32,i64)> = sqlx::query_as(
        "SELECT id,revision,enabled_ms FROM responsibilities
        WHERE status='active' AND expires_ms>? AND json_extract(plan_json,'$.trigger.kind')='event'
        AND json_extract(plan_json,'$.trigger.source')=? AND json_extract(plan_json,'$.trigger.eventType')=?",
    ).bind(ms).bind(&input.source).bind(&input.event_type).fetch_all(&mut *tx).await.map_err(internal)?;
    let mut admitted = 0;
    for (id, revision, enabled_at) in rows {
        use jeeves_state::responsibility_runs::{admit_on, Occurrence};
        let key = format!(
            "responsibility:{id}:event:{}:{}:{}",
            input.source, input.event_type, input.event_id
        );
        let recent = occurred > enabled_at && ms - occurred <= 900_000;
        let room = queued + i64::from(admitted) < 64;
        let skip = if !recent {
            Some("Event predates activation or exceeds the catch-up window")
        } else if !room {
            Some("The workspace queue is full")
        } else {
            None
        };
        if admit_on(
            &mut tx,
            Occurrence {
                id: &id,
                key: &key,
                revision,
                due_ms: Some(occurred),
                data: skip.is_none().then_some(body.as_str()),
                skip,
            },
        )
        .await
        .map_err(internal)?
            && skip.is_none()
        {
            admitted += 1;
        }
    }
    tx.commit().await.map_err(internal)?;
    eprintln!(
        "(jeeves) event on {} settled; {admitted} responsibilities queued",
        input.source
    );
    Ok(ResponsibilityEventReceipt {
        admitted,
        duplicate: false,
    })
}
