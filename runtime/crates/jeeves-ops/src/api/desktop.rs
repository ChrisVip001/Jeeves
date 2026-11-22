//! Authenticated cockpit adapters for task-scoped, operator-selected windows.
use super::preferences::relay;
use crate::AppServer;
use axum::{
    extract::{Extension, Query},
    response::Response,
    Json,
};
use jeeves_protocol::*;
use serde_json::json;
use std::sync::Arc;
pub async fn status(
    Extension(app): Extension<Arc<AppServer>>,
    Query(p): Query<TaskGetParams>,
) -> Response {
    relay::<DesktopStatus>(&app, methods::DESKTOP_STATUS, json!(p)).await
}
pub async fn select(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<TaskGetParams>,
) -> Response {
    relay::<DesktopObservation>(&app, methods::DESKTOP_SELECT, json!(p)).await
}
pub async fn observe(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<TaskGetParams>,
) -> Response {
    relay::<DesktopObservation>(&app, methods::DESKTOP_OBSERVE, json!(p)).await
}
pub async fn act(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DesktopActParams>,
) -> Response {
    relay::<DesktopObservation>(&app, methods::DESKTOP_ACT, json!(p)).await
}
pub async fn takeover(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<TaskGetParams>,
) -> Response {
    relay::<DesktopStatus>(&app, methods::DESKTOP_TAKEOVER, json!(p)).await
}
pub async fn model(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DesktopModelParams>,
) -> Response {
    relay::<DesktopStatus>(&app, methods::DESKTOP_MODEL, json!(p)).await
}
pub async fn frame(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DesktopFrameParams>,
) -> Response {
    relay::<DesktopFrame>(&app, methods::DESKTOP_FRAME, json!(p)).await
}
pub async fn share(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DesktopShareParams>,
) -> Response {
    relay::<DesktopAdvice>(&app, methods::DESKTOP_SHARE, json!(p)).await
}
