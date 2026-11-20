//! Tasks HTTP endpoints.

use super::failed;
use crate::AppServer;
use axum::extract::{Extension, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Json;
use jeeves_protocol::{
    codes, methods, ErrorObject, TaskActionParams, TaskCreateParams, TaskEnvelope, TaskListResponse,
};
use serde_json::json;
use std::sync::Arc;

pub async fn task_integrate(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<jeeves_protocol::TaskIntegrateParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::TaskGitCheckpointResponse>(
        &app,
        methods::TASK_INTEGRATE,
        json!(params),
    )
    .await
}

pub async fn task_git_checkpoint(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<jeeves_protocol::TaskGitCheckpointParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::TaskGitCheckpointResponse>(
        &app,
        methods::TASK_GIT_CHECKPOINT,
        json!(params),
    )
    .await
}

pub async fn task_git_checkpoint_policy(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<jeeves_protocol::TaskGetParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::TaskGitCheckpointPolicyResponse>(
        &app,
        methods::TASK_GIT_CHECKPOINT_POLICY,
        json!(params),
    )
    .await
}
pub async fn task_git_checkpoint_policy_set(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<jeeves_protocol::TaskGitCheckpointPolicyParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::TaskGitCheckpointPolicyResponse>(
        &app,
        methods::TASK_GIT_CHECKPOINT_POLICY_SET,
        json!(params),
    )
    .await
}

/// `GET /api/tasks` — the long-task panel: every task, newest first.
pub async fn tasks(
    Extension(app): Extension<Arc<AppServer>>,
    Query(page): Query<jeeves_protocol::TaskListParams>,
) -> Response {
    match app
        .request::<TaskListResponse>(methods::TASK_LIST, json!(page))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::BAD_GATEWAY, error),
    }
}

/// `GET /api/task/events?cursor=&taskId=&limit=` — the timeline after a cursor (a relay of `task/events`).
pub async fn task_events(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<jeeves_protocol::TaskEventsParams>,
) -> Response {
    match app
        .request::<jeeves_protocol::TaskEventsResponse>(methods::TASK_EVENTS, json!(params))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::BAD_REQUEST, error),
    }
}

/// `POST /api/task` — create / suspend / resume / cancel, one door for the human's switches.
///
/// The body carries `action` plus that action's params; ops only translates — every rule
/// (one active task per session, paused checkpoints stay resumable) lives in app-server.
pub async fn task(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let action = body
        .get("action")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string();
    if action == "artifacts" {
        return match app
            .request::<jeeves_protocol::TaskArtifactsResponse>(methods::TASK_ARTIFACTS, body)
            .await
        {
            Ok(response) => Json(response).into_response(),
            Err(error) => failed(StatusCode::OK, error),
        };
    }
    if matches!(action.as_str(), "operations" | "reconcile") {
        let translated = if action == "operations" {
            serde_json::from_value::<jeeves_protocol::TaskGetParams>(body).map(|params| {
                (
                    methods::TASK_OPERATIONS,
                    serde_json::to_value(params).expect("serialisable"),
                )
            })
        } else {
            serde_json::from_value::<jeeves_protocol::TaskReconcileParams>(body).map(|params| {
                (
                    methods::TASK_RECONCILE,
                    serde_json::to_value(params).expect("serialisable"),
                )
            })
        };
        let result = match translated {
            Ok((method, payload)) => {
                app.request::<jeeves_protocol::TaskOperationsResponse>(method, payload)
                    .await
            }
            Err(error) => Err(ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("Bad operation parameters: {error}"),
                data: None,
            }),
        };
        return match result {
            Ok(response) => Json(response).into_response(),
            Err(error) => failed(StatusCode::OK, error),
        };
    }
    if action == "signal" {
        let result = match serde_json::from_value::<jeeves_protocol::TaskSignalParams>(body) {
            Ok(params) => {
                app.request::<jeeves_protocol::TaskSignalResponse>(
                    methods::TASK_SIGNAL,
                    serde_json::to_value(params).expect("serialisable"),
                )
                .await
            }
            Err(error) => Err(ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("Bad task-input parameters: {error}"),
                data: None,
            }),
        };
        return match result {
            Ok(response) => Json(response).into_response(),
            Err(error) => failed(StatusCode::OK, error),
        };
    }
    let translated: Result<(&str, serde_json::Value), ErrorObject> = match action.as_str() {
        "wait" => serde_json::from_value::<jeeves_protocol::TaskWaitParams>(body.clone())
            .map(|params| {
                (
                    methods::TASK_WAIT,
                    serde_json::to_value(params).expect("serialisable"),
                )
            })
            .map_err(|error| ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("Bad task-wait parameters: {error}"),
                data: None,
            }),
        "start" => serde_json::from_value::<jeeves_protocol::TaskStartParams>(body.clone())
            .map(|params| {
                (
                    methods::TASK_START,
                    serde_json::to_value(params).expect("serialisable"),
                )
            })
            .map_err(|e| ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("Bad task-start parameters: {e}"),
                data: None,
            }),
        "create" => serde_json::from_value::<TaskCreateParams>(body.clone())
            .map(|params| {
                (
                    methods::TASK_CREATE,
                    serde_json::to_value(params).expect("serialisable"),
                )
            })
            .map_err(|e| ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("bad task-create parameters: {e}"),
                data: None,
            }),
        "suspend" | "resume" | "cancel" => serde_json::from_value::<TaskActionParams>(body.clone())
            .map(|params| {
                let method = match action.as_str() {
                    "suspend" => methods::TASK_SUSPEND,
                    "resume" => methods::TASK_RESUME,
                    _ => methods::TASK_CANCEL,
                };
                (method, serde_json::to_value(params).expect("serialisable"))
            })
            .map_err(|e| ErrorObject {
                code: codes::INVALID_PARAMS,
                message: format!("bad task-action parameters: {e}"),
                data: None,
            }),
        other => Err(ErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!(
                "unknown task action: {other} (create/start/wait/signal/operations/reconcile/suspend/resume/cancel)"
            ),
            data: None,
        }),
    };
    let result = match translated {
        Ok((method, payload)) => app.request::<TaskEnvelope>(method, payload).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(envelope) => Json(envelope).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}

/// Human inspection/reconciliation of stopped ordinary-chat tool operations.
pub async fn turn_operation(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let action = body
        .get("action")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let translated = match action {
        "operations" => serde_json::from_value::<jeeves_protocol::TurnOperationsParams>(
            body.clone(),
        )
        .map(|params| {
            (
                methods::TURN_OPERATIONS,
                serde_json::to_value(params).expect("serialisable"),
            )
        }),
        "reconcile" => serde_json::from_value::<jeeves_protocol::TurnReconcileParams>(body.clone())
            .map(|params| {
                (
                    methods::TURN_RECONCILE,
                    serde_json::to_value(params).expect("serialisable"),
                )
            }),
        _ => {
            return failed(
                StatusCode::OK,
                ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message: "Unknown chat operation action".into(),
                    data: None,
                },
            )
        }
    };
    let result = match translated {
        Ok((method, params)) => {
            app.request::<jeeves_protocol::TaskOperationsResponse>(method, params)
                .await
        }
        Err(error) => Err(ErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!("Invalid chat operation parameters: {error}"),
            data: None,
        }),
    };
    match result {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}
