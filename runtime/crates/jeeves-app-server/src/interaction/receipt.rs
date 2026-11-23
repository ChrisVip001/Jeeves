use super::*;
use crate::Frame;
use serde_json::{Value, json};

pub(crate) async fn replay(
    server: &Server,
    (session, key): (&str, &str),
    input: &str,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    let receipt = jeeves_state::interactions::get(&server.pool, session, key)
        .await
        .map_err(internal)?;
    if receipt.input_json != input {
        return Err(invalid("Message identity was reused with different input"));
    }
    if receipt.state == "claimed" {
        let rows: Vec<(String,String)> = sqlx::query_as("SELECT t.id,t.title FROM conversation_handoffs h JOIN agent_tasks t ON t.id=h.task_id WHERE h.session_id=? AND h.message_id=? UNION ALL SELECT t.id,t.title FROM task_amendments a JOIN agent_tasks t ON t.id=a.task_id WHERE t.session_id=? AND a.message_id=?")
            .bind(session).bind(key).bind(session).bind(key).fetch_all(&server.pool).await.map_err(internal)?;
        if !rows.is_empty() {
            let p: InteractionStartParams = serde_json::from_str(input).map_err(internal)?;
            let tasks = rows
                .iter()
                .map(|(id, title)| format!("{title} ({id})"))
                .collect::<Vec<_>>()
                .join("、");
            let reply = format!("已找回这条消息提交到的事项：{tasks}。请在原事项查看进度。");
            return acknowledge(server, &p, id, sink, reply, Instant::now()).await;
        }
    }
    if receipt.state != "completed" {
        return Err(invalid(receipt.error.unwrap_or_else(|| {
            "Previous submission is unfinished; inspect task progress before resubmitting".into()
        })));
    }
    let events: Vec<ServerNotification> =
        serde_json::from_str(receipt.events_json.as_deref().unwrap_or("[]")).map_err(internal)?;
    for event in events {
        sink.notification(event);
    }
    let result = serde_json::from_str(
        receipt
            .result_json
            .as_deref()
            .ok_or_else(|| internal("Missing interaction receipt"))?,
    )
    .map_err(internal)?;
    respond(sink, id, result);
    Ok(())
}

/// Existing engine notifications remain live; the response follows durable receipt commit.
pub(super) async fn conversation(
    server: &Server,
    p: &InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
    interaction_tool: Option<&jeeves_tools::Tool>,
    tier: Option<String>,
) -> Result<(), ErrorObject> {
    let params = TurnStartParams {
        session_id: Some(p.session_id.clone()),
        message: p.message.clone(),
        stream: Some(true),
        cwd: p.cwd.clone(),
        tier,
        attachments: p.attachments.clone(),
        ..Default::default()
    };
    recorded(
        server,
        (&p.session_id, &p.message_id),
        params,
        id,
        sink,
        interaction_tool,
        p.source.is_some(),
    )
    .await
}

/// Run one turn and keep its result and start/finish notifications under `(session, key)`,
/// so a retry of the same message replays them instead of running again.
pub(crate) async fn recorded(
    server: &Server,
    (session, key): (&str, &str),
    params: TurnStartParams,
    id: RequestId,
    sink: &EventSink,
    interaction_tool: Option<&jeeves_tools::Tool>,
    channel: bool,
) -> Result<(), ErrorObject> {
    let (outlet, mut frames) = EventSink::channel();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let target = sink.clone();
    tokio::spawn(async move {
        let mut events = Vec::new();
        let mut sender = Some(sender);
        while let Some(frame) = frames.recv().await {
            match frame {
                Frame::Notification(event) => {
                    if matches!(
                        event,
                        ServerNotification::TurnStarted(_) | ServerNotification::TurnCompleted(_)
                    ) {
                        events.push(event.clone());
                    }
                    target.notification(event);
                }
                Frame::Response(JsonRpcMessage::Response(response)) => {
                    if let Some(sender) = sender.take() {
                        let _ = sender.send((response.result, events.clone()));
                    }
                }
                Frame::Response(response) => target.response(response),
            }
        }
    });
    crate::turn::run_turn_inner(
        server,
        params,
        id.clone(),
        &outlet,
        None,
        interaction_tool,
        Some((key, channel)),
    )
    .await?;
    drop(outlet);
    let (result, events) = receiver.await.map_err(internal)?;
    finish(server, (session, key), &result, &events).await?;
    respond(sink, id, result);
    Ok(())
}
pub(super) async fn acknowledge(
    server: &Server,
    p: &InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
    reply: String,
    started: Instant,
) -> Result<(), ErrorObject> {
    let turn_id = jeeves_protocol::interaction_turn_id(&p.message_id, p.source.is_some());
    let resolved = server
        .resolved()
        .ok_or_else(|| invalid("Configure a model before sending a message"))?;
    let meta = TurnMeta {
        iterations: 0,
        latency_ms: crate::narrow(started.elapsed().as_millis() as i64),
        tools: vec![],
        model: resolved.small_model,
        provider: resolved.provider_id,
        usage: None,
        interrupted: false,
        guard_hits: 0,
        guard_note: None,
        retries: 0,
        goal_status: None,
        goal_rounds: 0,
        route_reason: None,
    };
    server
        .chat
        .append_exchange(
            &p.message,
            &reply,
            &p.session_id,
            "interaction",
            Some(&serde_json::to_string(&meta).map_err(internal)?),
        )
        .await
        .map_err(internal)?;
    let settings = server.settings();
    let ledger_sink = crate::ledger_writer::LedgerSink::new(
        &settings.home,
        settings.ledger_inline_max.max(0) as usize,
    );
    let entries = crate::ledger_writer::entries_for_turn(
        &ledger_sink,
        &p.session_id,
        &turn_id,
        &p.message,
        &[],
        &reply,
    );
    if let Err(error) = server
        .ledger
        .append(&p.session_id, &turn_id, &entries, &p.message)
        .await
    {
        eprintln!("(jeeves) interaction ledger write failed: {error}");
    }
    let events = vec![
        ServerNotification::TurnStarted(TurnStartedNotification {
            turn_id: turn_id.clone(),
            session_id: p.session_id.clone(),
            user_message: p.message.clone(),
            ts: chrono::Local::now().to_rfc3339(),
        }),
        ServerNotification::TurnCompleted(Box::new(TurnCompletedNotification {
            turn_id: turn_id.clone(),
            reply,
            iterations: 0,
            usage: meta.usage.clone(),
            meta,
        })),
    ];
    let result = json!({"turnId":turn_id});
    finish(server, (&p.session_id, &p.message_id), &result, &events).await?;
    for event in events {
        sink.notification(event);
    }
    respond(sink, id, result);
    Ok(())
}
async fn finish(
    server: &Server,
    (session, key): (&str, &str),
    result: &Value,
    events: &[ServerNotification],
) -> Result<(), ErrorObject> {
    jeeves_state::interactions::complete(
        &server.pool,
        session,
        key,
        &result.to_string(),
        &serde_json::to_string(events).map_err(internal)?,
    )
    .await
    .map_err(internal)
}
fn respond(sink: &EventSink, id: RequestId, result: Value) {
    sink.response(JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result,
    }));
}
