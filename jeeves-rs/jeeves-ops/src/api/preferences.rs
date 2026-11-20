//! Typed, authenticated HTTP adapters for shared desktop/Web controls.
use super::failed;
use crate::AppServer;
use axum::{
    extract::{Extension, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use jeeves_protocol::{
    methods, ConfigReadResponse, ConfigWriteParams, ConfigWriteResponse, ModelListResponse,
    SessionPermissionGetParams, SessionPermissionResponse, SessionPermissionSetParams,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) async fn relay<T: DeserializeOwned + serde::Serialize>(
    app: &AppServer,
    method: &str,
    params: Value,
) -> Response {
    match app.request::<T>(method, params).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::BAD_REQUEST, error),
    }
}
pub async fn permission_get(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<SessionPermissionGetParams>,
) -> Response {
    relay::<SessionPermissionResponse>(&app, methods::SESSION_PERMISSION_GET, json!(params)).await
}
pub async fn permission_set(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<SessionPermissionSetParams>,
) -> Response {
    relay::<SessionPermissionResponse>(&app, methods::SESSION_PERMISSION_SET, json!(params)).await
}
pub async fn config_get(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<ConfigReadResponse>(&app, methods::CONFIG_READ, json!({})).await
}
pub async fn config_set(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<ConfigWriteParams>,
) -> Response {
    relay::<ConfigWriteResponse>(&app, methods::CONFIG_WRITE, json!(params)).await
}
/// `POST /api/memory/remember`: the first-run wizard writes what the owner says about themself.
pub async fn remember(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<jeeves_protocol::MemoryRememberParams>,
) -> Response {
    relay::<jeeves_protocol::MemoryRememberResponse>(&app, methods::MEMORY_REMEMBER, json!(params))
        .await
}
pub async fn model_catalog(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<ModelListResponse>(&app, methods::MODEL_LIST, json!({})).await
}

pub async fn main_session(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<jeeves_protocol::SessionNewResponse>(&app, methods::SESSION_MAIN, json!({})).await
}

pub async fn pending_approvals(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<jeeves_protocol::ApprovalPendingParams>,
) -> Response {
    relay::<jeeves_protocol::ApprovalPendingResponse>(
        &app,
        methods::APPROVAL_PENDING,
        json!(params),
    )
    .await
}
