//! Pair and revoke through the operator transport. Reports carry their own device proof.
use super::preferences::relay;
use crate::{AppServer, Gate};
use axum::{
    extract::{Extension, Query, State},
    response::Response,
    Json,
};
use jeeves_protocol::*;
use serde_json::json;
use std::sync::Arc;

pub async fn list(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<DeviceListResponse>(&app, methods::DEVICE_LIST, json!({})).await
}
pub async fn pair(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DevicePairParams>,
) -> Response {
    relay::<DeviceGrantView>(&app, methods::DEVICE_PAIR, json!(p)).await
}
pub async fn revoke(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<DeviceRevokeParams>,
) -> Response {
    relay::<DeviceGrantView>(&app, methods::DEVICE_REVOKE, json!(p)).await
}
pub async fn report(State(gate): State<Gate>, Json(p): Json<DeviceReportParams>) -> Response {
    relay::<DeviceReportResponse>(&gate.app(), methods::DEVICE_REPORT, json!(p)).await
}

#[derive(serde::Deserialize)]
pub struct NoticeProof {
    signature: String,
}
pub async fn notice(
    Extension(app): Extension<Arc<AppServer>>,
    Query(notice): Query<ApprovalNoticeParams>,
    Query(proof): Query<NoticeProof>,
) -> Response {
    let p = ApprovalNoticeCheckParams {
        notice,
        signature: proof.signature,
    };
    relay::<ApprovalNoticeView>(&app, methods::APPROVAL_NOTICE, json!(p)).await
}
pub async fn decision(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<ApprovalNoticeDecisionParams>,
) -> Response {
    relay::<ApprovalRespondResponse>(&app, methods::APPROVAL_NOTICE_DECISION, json!(p)).await
}
pub async fn subscribe(
    Extension(app): Extension<Arc<AppServer>>,
    Json(p): Json<ApprovalPushSubscribeParams>,
) -> Response {
    relay::<DeviceReportResponse>(&app, methods::APPROVAL_PUSH_SUBSCRIBE, json!(p)).await
}
pub async fn push_key(Extension(app): Extension<Arc<AppServer>>) -> Response {
    relay::<ApprovalPushKeyResponse>(&app, methods::APPROVAL_PUSH_KEY, json!({})).await
}
