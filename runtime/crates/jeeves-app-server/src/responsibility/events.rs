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
                .map(|s| serde_json::json!({"source":s.name,"eventType":s.event_type}))
                .collect(),
        ),
        Err(error) => serde_json::json!({"blocked":error.to_string()}),
    }
}

fn enabled(server: &Server, source: &str, event_type: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        sources(server)?
            .iter()
            .any(|entry| entry.name == source && entry.event_type.as_deref() == Some(event_type)),
        "Connect and enable the signed event source {source} ({event_type})"
    );
    Ok(())
}

pub(super) fn preflight(server: &Server, trigger: &ResponsibilityTrigger) -> anyhow::Result<()> {
    if let ResponsibilityTrigger::Event { source, event_type } = trigger {
        enabled(server, source, event_type)?;
    }
    Ok(())
}

pub(crate) async fn admit(
    server: &Server,
    input: ResponsibilityEvent,
    now: DateTime<Utc>,
) -> Result<ResponsibilityEventReceipt, ErrorObject> {
    enabled(server, &input.source, &input.event_type).map_err(invalid)?;
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
    let rows: Vec<(String,i32,i64)> = sqlx::query_as(
        "SELECT id,revision,enabled_ms FROM responsibilities
        WHERE status='active' AND expires_ms>? AND json_extract(plan_json,'$.trigger.kind')='event'
        AND json_extract(plan_json,'$.trigger.source')=? AND json_extract(plan_json,'$.trigger.eventType')=?",
    ).bind(ms).bind(&input.source).bind(&input.event_type).fetch_all(&mut *tx).await.map_err(internal)?;
    let mut admitted = 0;
    for (id, revision, enabled_at) in rows {
        let target = format!("responsibility:{id}");
        let key = format!(
            "{target}:event:{}:{}:{}",
            input.source, input.event_type, input.event_id
        );
        let recent = occurred > enabled_at && ms - occurred <= 900_000;
        let inserted = sqlx::query("INSERT INTO wake_inbox(source_key,target_task,responsibility_revision,due_ms,event_json,state)
            VALUES(?,?,?,?,?,?) ON CONFLICT(source_key) DO NOTHING")
            .bind(&key).bind(&target).bind(revision).bind(occurred).bind(recent.then_some(&body))
            .bind(if recent {"pending"} else {"delivered"}).execute(&mut *tx).await.map_err(internal)?.rows_affected()==1;
        if inserted && recent {
            admitted += 1;
        } else if inserted {
            sqlx::query("INSERT INTO responsibility_runs(wake_key,responsibility_id,status,outcome,detail)
                VALUES(?,?,'draft','skipped','Event predates activation or exceeds the catch-up window')")
                .bind(&key).bind(id).execute(&mut *tx).await.map_err(internal)?;
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
