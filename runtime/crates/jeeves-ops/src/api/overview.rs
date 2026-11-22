//! Overview HTTP endpoints.

use super::failed;
use crate::AppServer;
use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Json;
use chrono::Local;
use jeeves_protocol::{
    methods, ApprovalLogResponse, ConfigReadResponse, ConnectorStatus, DashboardData, ErrorObject,
    MemoryListEpisodesResponse, MemoryListResponse, SessionListResponse, TaskArtifactsResponse,
    TaskViewResponse, TodoListView, UsageSummaryResponse,
};
use serde_json::json;
use std::sync::Arc;

/// The first screen fetches only this many recent entries: the dashboard must show "the
/// state right now" at a glance; paging is another matter (the protocol cursor is for that).
pub(super) const SESSION_LIMIT: u32 = 20;

pub(super) const FACT_LIMIT: u32 = 30;

pub(super) const EPISODE_LIMIT: u32 = 20;

/// `GET /api/data` — everything the first screen needs, fetched in one go.
///
/// One endpoint instead of four concurrent frontend requests: the app-server handles one
/// request at a time (over stdio it is just a loop), so four requests queue there anyway.
/// Composing one also guarantees "these numbers are from the same moment".
pub async fn data(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Response {
    match collect(&app, params.get("sessionId").map(String::as_str)).await {
        Ok(data) => Json(data).into_response(),
        Err(error) => failed(StatusCode::BAD_GATEWAY, error),
    }
}

pub(super) async fn collect(
    app: &AppServer,
    session: Option<&str>,
) -> Result<DashboardData, ErrorObject> {
    let config: ConfigReadResponse = app.request(methods::CONFIG_READ, json!({})).await?;
    let sessions: SessionListResponse = app
        .request(methods::SESSION_LIST, json!({ "limit": SESSION_LIMIT }))
        .await?;
    let facts: MemoryListResponse = app
        .request(methods::MEMORY_LIST, json!({ "limit": FACT_LIMIT }))
        .await?;
    let episodes: MemoryListEpisodesResponse = app
        .request(
            methods::MEMORY_LIST_EPISODES,
            json!({ "limit": EPISODE_LIMIT }),
        )
        .await?;

    let (connectors, connectors_error) = connector_status(app);
    let (todos, task, task_artifacts) = match session {
        Some(id) => {
            let todos: Option<TodoListView> = app
                .request(methods::TODO_VIEW, json!({ "sessionId": id }))
                .await?;
            let task: TaskViewResponse = app
                .request(methods::TASK_VIEW, json!({ "sessionId": id }))
                .await?;
            let task_artifacts = match &task.task {
                Some(row) => app
                    .request::<TaskArtifactsResponse>(
                        methods::TASK_ARTIFACTS,
                        json!({ "taskId": row.id }),
                    )
                    .await
                    .map(|response| response.artifacts)
                    .unwrap_or_else(|error| {
                        eprintln!("(jeeves) could not read the task deliverables (the panel shows them empty): {}", error.message);
                        Vec::new()
                    }),
                None => Vec::new(),
            };
            (todos, task.task, task_artifacts)
        }
        None => (None, None, Vec::new()),
    };

    Ok(DashboardData {
        generated_at: Local::now().to_rfc3339(),
        config: config.config,
        sessions: sessions.data,
        facts: facts.data,
        episodes: episodes.data,
        todos,
        task,
        task_artifacts,
        connectors,
        connectors_error,
    })
}

/// `POST /api/approval` — answer one "should this run".
/// What the connector panel shows: one row per configured connector, or the reason the
/// file could not be read. The allowlist size is the visible width of the door.
pub(super) fn connector_status(app: &AppServer) -> (Vec<ConnectorStatus>, Option<String>) {
    match jeeves_connector::config::load(app.home()) {
        Ok(config) => {
            let mut rows = Vec::new();
            if let Some(feishu) = config.feishu {
                rows.push(ConnectorStatus {
                    id: "feishu".to_string(),
                    live: feishu.live,
                    allow_recipients: feishu.allow_recipients.len(),
                });
            }
            (rows, None)
        }
        Err(e) => (Vec::new(), Some(e.to_string())),
    }
}

///
/// The payload is the protocol's `ApprovalRespondParams`. **Not SSE**: one question, one
/// answer, then gone. Too late → `accepted: false` (the turn stopped waiting long ago);
/// the frontend knows its click went unheard instead of assuming the approval took effect.
/// Configured model profiles + the current one (**no keys**, only a "where is the key" ptr).
///
/// Same discipline as `/api/task`: these are **human-only** settings — the model has no
/// `model/*` tools and cannot reach the keys.
/// Usage ledger (migration 0010): per-model aggregates + totals. Bookkeeping, not enforcing.
pub async fn usage(Extension(app): Extension<Arc<AppServer>>) -> Response {
    match app
        .request::<UsageSummaryResponse>(methods::USAGE_SUMMARY, json!({}))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}

/// Approval audit (migration 0011): the last 100 "asked before running", newest first.
pub async fn approvals(Extension(app): Extension<Arc<AppServer>>) -> Response {
    match app
        .request::<ApprovalLogResponse>(methods::APPROVAL_LOG, json!({}))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}
