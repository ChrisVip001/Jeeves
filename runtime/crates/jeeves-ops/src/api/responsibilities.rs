//! Human cockpit controls. The app-server validates and owns the authorization.
use super::preferences::relay;
use crate::AppServer;
use axum::{extract::Extension, response::Response, Json};
use jeeves_protocol::{methods, ResponsibilityListResponse, ResponsibilityView};
use serde_json::{json, Value};
use std::sync::Arc;

pub async fn list(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<ResponsibilityListResponse>(&app, methods::RESPONSIBILITY_LIST, json!({})).await
}

pub async fn prepare(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<Value>,
) -> Response {
    relay::<jeeves_protocol::ResponsibilityContract>(&app, methods::RESPONSIBILITY_PREPARE, body)
        .await
}

pub async fn amend(Extension(app): Extension<Arc<AppServer>>, Json(body): Json<Value>) -> Response {
    relay::<ResponsibilityView>(&app, methods::RESPONSIBILITY_AMEND, body).await
}

pub async fn register(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<Value>,
) -> Response {
    relay::<ResponsibilityView>(&app, methods::RESPONSIBILITY_REGISTER, body).await
}

pub async fn set_paused(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<Value>,
) -> Response {
    relay::<ResponsibilityView>(&app, methods::RESPONSIBILITY_SET_PAUSED, body).await
}

pub async fn reports(
    Extension(app): Extension<Arc<AppServer>>,
    axum::extract::Query(params): axum::extract::Query<
        jeeves_protocol::ResponsibilityReportsParams,
    >,
) -> Response {
    relay::<jeeves_protocol::ResponsibilityReportsResponse>(
        &app,
        methods::RESPONSIBILITY_REPORTS,
        json!(params),
    )
    .await
}
