//! Stream HTTP endpoints.

use super::failed;
use crate::AppServer;
use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures_util::stream::{self, Stream, StreamExt};
use jeeves_protocol::{
    ErrorObject, RunResponse, ServerNotification, TurnStartParams, TurnStartResponse, codes,
    interaction_turn_id, methods,
};
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

/// How many not-yet-consumed notifications may pile up. **Unbounded** on purpose: bounded
/// would stall the forwarding task on send, leaving the broadcast channel unconsumed —
/// intermediate `textDelta`s get dropped, possibly along with `turnCompleted`, and that
/// SSE turn would never end. The unbounded cap is one full turn's worth of notifications,
/// and the channel is torn down when the turn ends.
pub(super) const TURN_BUFFER: usize = 1024;

/// Cross-device sync ping: a notification distilled to "what happened, where". A ping is
/// a knock on the door, **not the mail** — the receiver pulls `/api/data` for details.
/// Deliberately narrow: textDelta / tool-call floods stay inside the owning device's own
/// turn stream; other devices only learn that *something* happened.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventPing {
    pub kind: &'static str,
    pub turn_id: Option<String>,
    pub session_id: Option<String>,
    /// Newest task-event cursor when the knock concerns a task: the client compares it with
    /// its own and calls `task/events` only when behind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i32>,
}

impl EventPing {
    pub(crate) fn of(notification: &ServerNotification) -> Option<EventPing> {
        let ping = match notification {
            ServerNotification::TurnStarted(n) => EventPing {
                kind: "turn_started",
                turn_id: Some(n.turn_id.clone()),
                session_id: Some(n.session_id.clone()),
                cursor: None,
            },
            ServerNotification::TurnCompleted(n) => EventPing {
                kind: "turn_completed",
                turn_id: Some(n.turn_id.clone()),
                session_id: None,
                cursor: None,
            },
            ServerNotification::ApprovalRequested(n) => EventPing {
                kind: "approval",
                turn_id: Some(n.turn_id.clone()),
                session_id: None,
                cursor: None,
            },
            ServerNotification::ConsolidationCompleted(_) => EventPing {
                kind: "memory",
                turn_id: None,
                session_id: None,
                cursor: None,
            },
            ServerNotification::Error(_) => EventPing {
                kind: "error",
                turn_id: None,
                session_id: None,
                cursor: None,
            },
            ServerNotification::TurnCheckpointed(n) => EventPing {
                kind: "task_progress",
                turn_id: Some(n.turn_id.clone()),
                session_id: Some(n.session_id.clone()),
                cursor: None,
            },
            ServerNotification::TurnResumed(n) => EventPing {
                kind: "task_resumed",
                turn_id: Some(n.turn_id.clone()),
                session_id: Some(n.session_id.clone()),
                cursor: None,
            },
            ServerNotification::TurnSuspended(n) => EventPing {
                kind: "task_suspended",
                turn_id: Some(n.turn_id.clone()),
                session_id: Some(n.session_id.clone()),
                cursor: None,
            },
            ServerNotification::TaskUpdated(n) => EventPing {
                kind: "task",
                turn_id: None,
                session_id: n.session_id.clone(),
                cursor: None,
            },
            _ => return None,
        };
        Some(ping)
    }
}

/// The newest task-event cursor, or `None` when it cannot be read (the knock still goes out).
async fn latest_cursor(app: &AppServer) -> Option<i32> {
    let page: jeeves_protocol::TaskEventsResponse = app
        .request(
            methods::TASK_EVENTS,
            json!({ "cursor": i32::MAX, "limit": 1 }),
        )
        .await
        .ok()?;
    Some(page.latest_cursor)
}

/// `GET /api/events` — one SSE channel of pings for this session's app-server.
///
/// Every open view (another tab, the desktop shell) holds one, so a turn started in one view
/// shows up in the others.
pub async fn events(Extension(app): Extension<Arc<AppServer>>) -> Response {
    let stopping = app.stopping();
    let rx = app.subscribe();
    let stream = futures_util::stream::unfold((rx, app), |(mut rx, app)| async move {
        loop {
            match rx.recv().await {
                Ok(notification) => {
                    if let Some(mut ping) = EventPing::of(&notification) {
                        if ping.kind.starts_with("task") {
                            ping.cursor = latest_cursor(&app).await;
                        }
                        return Some((ping, (rx, app)));
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
    .map(|ping| {
        Ok::<_, std::convert::Infallible>(
            Event::default().data(serde_json::to_string(&ping).expect("ping serialises into JSON")),
        )
    });

    Sse::new(stream.take_until(stopping))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `POST /api/run` — **the task API**: one turn in, one answer out, plain JSON.
///
/// Accepts `TurnStartParams`, but no SSE: a program (CI,
/// cron, another agent) wants the reply, not a typewriter. The dashboard keeps SSE because a
/// person wants to watch the words arrive; this endpoint exists for callers that don't.
pub async fn run(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<TurnStartParams>,
) -> Response {
    if params.message.trim().is_empty() {
        return failed(
            StatusCode::BAD_REQUEST,
            ErrorObject {
                code: codes::INVALID_PARAMS,
                message: "the message is empty".to_string(),
                data: None,
            },
        );
    }
    match run_headless(&app, params).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}

fn prepare_public_turn(params: &mut TurnStartParams) -> String {
    let request = params.request_id.get_or_insert_with(|| {
        format!(
            "http-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        )
    });
    interaction_turn_id(&format!("turn-{request}"), false)
}

/// One turn in, one answer out, shared by the JSON endpoint and inbound webhooks.
pub(crate) async fn run_headless(
    app: &Arc<AppServer>,
    mut params: TurnStartParams,
) -> Result<RunResponse, ErrorObject> {
    let expected = prepare_public_turn(&mut params);
    let payload = serde_json::to_value(&params).expect("TurnStartParams always serialises");
    let mut events = app.subscribe();
    let request = app.request::<TurnStartResponse>(methods::TURN_START, payload);
    tokio::pin!(request);
    let response = loop {
        tokio::select! {
            response = &mut request => break response?,
            event = events.recv() => match event {
                Ok(notification) if turn_id_of(&notification) == Some(expected.as_str()) => {
                    if let Some(run) = completed_turn(&notification) {
                        request.await?;
                        return Ok(run);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break request.await?,
                _ => {}
            }
        }
    };
    while let Ok(notification) = events.try_recv() {
        if turn_id_of(&notification) == Some(expected.as_str())
            && let Some(run) = completed_turn(&notification)
        {
            return Ok(run);
        }
    }
    Err(ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: format!(
            "The turn {} finished without a matching reply",
            response.turn_id
        ),
        data: None,
    })
}

/// Forward exactly the originating interaction's notifications to this stream.
async fn forward_method(
    app: &AppServer,
    payload: serde_json::Value,
    (session_id, expected): (String, String),
    tx: mpsc::Sender<ServerNotification>,
    mut events: broadcast::Receiver<ServerNotification>,
    method: &str,
) {
    let request = app.request::<TurnStartResponse>(method, payload);
    tokio::pin!(request);
    let mut answered = false;
    let mut turn_id: Option<String> = None;

    loop {
        tokio::select! {
            _ = tx.closed() => break,
            result = &mut request, if !answered => {
                answered = true;
                if let Err(error) = result {
                    let _ = tx.send(crate::as_notification(error)).await;
                    break;
                }
            }
            received = events.recv() => {
                let notification = match received {
                    Ok(notification) => notification,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        eprintln!("(jeeves) notifications came too fast; {missed} were dropped this turn");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };

                if matches!(&notification, ServerNotification::TurnStarted(started) if started.turn_id != expected) {
                    continue;
                }
                let Some(id) = belongs(&notification, &session_id, turn_id.as_deref()) else {
                    continue;
                };
                turn_id = Some(id);

                let last = matches!(notification, ServerNotification::TurnCompleted(_));
                if tx.send(notification).await.is_err() {
                    break;
                }
                if last {
                    break;
                }
            }
        }
    }
}

/// `turnCompleted` is the one-shot answer: reply + which model actually ran. Anything else: None.
pub(super) fn completed_turn(notification: &ServerNotification) -> Option<RunResponse> {
    let ServerNotification::TurnCompleted(done) = notification else {
        return None;
    };
    Some(RunResponse {
        turn_id: done.turn_id.clone(),
        reply: done.reply.clone(),
        model: done.meta.model.clone(),
        provider: done.meta.provider.clone(),
        route_reason: done.meta.route_reason.clone(),
    })
}

/// Does this notification belong to this turn? If so, return its turnId.
///
/// The first frame, `turnStarted`, can only be recognized by sessionId (no turnId yet);
/// every later frame carries a turnId — id matching avoids mixing in "another session that
/// happens to be running". `consolidationCompleted` and `graph*` have no turnId; they are
/// forwarded only after this turn has already been claimed.
pub(super) fn belongs(
    notification: &ServerNotification,
    session_id: &str,
    known: Option<&str>,
) -> Option<String> {
    use ServerNotification as N;
    match notification {
        N::TurnStarted(started) if started.session_id == session_id && known.is_none() => {
            Some(started.turn_id.clone())
        }
        N::PermissionChanged(policy) if policy.session_id != session_id => None,
        _ if known.is_none() => None,
        other => match turn_id_of(other) {
            Some(id) if Some(id) == known => Some(id.to_string()),
            Some(_) => None,
            None => known.map(str::to_string),
        },
    }
}

/// The turnId carried in a notification. Variants without that field return None (see `belongs`).
pub(super) fn turn_id_of(notification: &ServerNotification) -> Option<&str> {
    use ServerNotification as N;
    match notification {
        N::TurnStarted(n) => Some(&n.turn_id),
        N::TextDelta(n) => Some(&n.turn_id),
        N::Retry(n) => Some(&n.turn_id),
        N::ApprovalRequested(n) => Some(&n.turn_id),
        N::GoalRound(n) => Some(&n.turn_id),
        N::ToolStarted(n) => Some(&n.turn_id),
        N::ToolDelta(n) => Some(&n.turn_id),
        N::ToolCompleted(n) => Some(&n.turn_id),
        N::TurnCompleted(n) => Some(&n.turn_id),
        N::TurnCheckpointed(n) => Some(&n.turn_id),
        N::TurnResumed(n) => Some(&n.turn_id),
        N::TurnSteered(n) => Some(&n.turn_id),
        N::TurnSuspended(n) => Some(&n.turn_id),
        N::PermissionChanged(_)
        | N::TaskUpdated(_)
        | N::ConsolidationCompleted(_)
        | N::Error(_) => None,
    }
}

/// Notification channel → SSE frames.
///
/// Each frame is the JSON of one notification (`data: {...}`); the frontend `JSON.parse`s
/// it and dispatches on the `type` field — an exact match for `ServerNotification`'s
/// discriminant, so the frontend needs no second schema.
pub(super) fn bytes_stream(
    rx: mpsc::Receiver<ServerNotification>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    stream::unfold(rx, |mut rx| async move {
        let notification = rx.recv().await?;
        let json = serde_json::to_string(&notification).expect("a notification always serialises");
        Some((Ok(Event::default().data(json)), rx))
    })
}

/// Human interaction stream; policy and task routing live entirely in app-server.
pub async fn interaction(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<jeeves_protocol::InteractionStartParams>,
) -> Response {
    if params.source.is_some() {
        return failed(
            StatusCode::BAD_REQUEST,
            ErrorObject {
                code: codes::INVALID_PARAMS,
                message: "Use the gateway entry for channel interactions".into(),
                data: None,
            },
        );
    }
    let events = app.subscribe();
    let stopping = app.stopping();
    let (tx, rx) = mpsc::channel::<ServerNotification>(TURN_BUFFER);
    let key = interaction_turn_id(&params.message_id, params.source.is_some());
    let payload = serde_json::to_value(&params).expect("interaction serialises");
    tokio::spawn(async move {
        forward_method(
            &app,
            payload,
            (params.session_id, key),
            tx,
            events,
            methods::INTERACTION_START,
        )
        .await;
    });
    Sse::new(bytes_stream(rx).take_until(stopping))
        .keep_alive(KeepAlive::default())
        .into_response()
}
/// Authenticated event data for task cards, from the one app-server this cockpit runs.
pub async fn notifications(Extension(app): Extension<Arc<AppServer>>) -> Response {
    let stopping = app.stopping();
    let stream = futures_util::stream::unfold(app.subscribe(), |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ServerNotification::TextDelta(_) | ServerNotification::ToolDelta(_)) => continue,
                Ok(event) => {
                    return Some((
                        Ok::<_, std::convert::Infallible>(
                            Event::default().data(
                                serde_json::to_string(&event).expect("notification serialises"),
                            ),
                        ),
                        rx,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    eprintln!("(jeeves) task notifications lagged by {missed}; refresh task state")
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream.take_until(stopping))
        .keep_alive(KeepAlive::default())
        .into_response()
}
