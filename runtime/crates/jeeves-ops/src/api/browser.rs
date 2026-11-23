//! Authenticated, typed transport only; browser policy lives in app-server.
use super::preferences::relay;
use crate::AppServer;
use axum::{
    Json,
    extract::{Extension, Query},
    response::Response,
};
use jeeves_protocol::*;
use serde_json::json;
use std::sync::Arc;

pub async fn list(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<BrowserListParams>,
) -> Response {
    relay::<BrowserListResponse>(&app, methods::BROWSER_LIST, json!(params)).await
}
pub async fn start(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<BrowserStartParams>,
) -> Response {
    relay::<BrowserSessionView>(&app, methods::BROWSER_START, json!(params)).await
}
pub async fn control(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<BrowserControlParams>,
) -> Response {
    relay::<BrowserSessionView>(&app, methods::BROWSER_CONTROL, json!(params)).await
}
pub async fn frame(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<TaskGetParams>,
) -> Response {
    relay::<BrowserFrameResponse>(&app, methods::BROWSER_FRAME, json!(params)).await
}

pub async fn executor_status(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<ExecutorControlView>(&app, methods::EXECUTOR_CONTROL, json!({})).await
}
pub async fn stop(
    Extension(app): Extension<Arc<AppServer>>,
    Json(_): Json<serde_json::Map<String, serde_json::Value>>,
) -> Response {
    relay::<ExecutorControlView>(&app, methods::EXECUTOR_STOP, json!({})).await
}
pub async fn release(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<ExecutorReleaseParams>,
) -> Response {
    relay::<ExecutorControlView>(&app, methods::EXECUTOR_RELEASE, json!(p)).await
}

pub async fn input(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<BrowserHumanInputParams>,
) -> Response {
    relay::<BrowserFrameResponse>(&app, methods::BROWSER_INPUT, json!(params)).await
}
pub async fn downloads(
    Extension(app): Extension<Arc<AppServer>>,
    Query(params): Query<TaskGetParams>,
) -> Response {
    relay::<BrowserDownloadsResponse>(&app, methods::BROWSER_DOWNLOADS, json!(params)).await
}
pub async fn download(
    Extension(app): Extension<Arc<AppServer>>,
    Json(params): Json<BrowserDownloadParams>,
) -> Response {
    relay::<BrowserDownloadsResponse>(&app, methods::BROWSER_DOWNLOAD, json!(params)).await
}
