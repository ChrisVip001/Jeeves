//! Settings → Connections: the Lark gateway the Runtime hosts.

use crate::gateways::{Gateways, LarkConfig};
use axum::{
    extract::Extension, http::StatusCode, response::IntoResponse, response::Response, Json,
};
use serde::Deserialize;
use std::sync::Arc;

pub async fn connector_settings(Extension(app): Extension<Arc<crate::AppServer>>) -> Response {
    super::preferences::relay::<jeeves_protocol::ConnectorSettingsResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_SETTINGS,
        serde_json::json!({}),
    )
    .await
}

pub async fn connector_settings_save(
    Extension(app): Extension<Arc<crate::AppServer>>,
    Json(params): Json<jeeves_protocol::ConnectorSettingsSaveParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::ConnectorSettingsResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_SETTINGS_SAVE,
        serde_json::to_value(params).expect("connector params serialize"),
    )
    .await
}

/// Relay capability cards without duplicating Runtime policy in the shell.
pub async fn capability_list(Extension(app): Extension<Arc<crate::AppServer>>) -> Response {
    super::preferences::relay::<jeeves_protocol::CapabilityListResponse>(
        &app,
        jeeves_protocol::methods::CAPABILITY_LIST,
        serde_json::json!({}),
    )
    .await
}

pub async fn connector_oauth_start(
    Extension(app): Extension<Arc<crate::AppServer>>,
    Json(params): Json<jeeves_protocol::ConnectorOAuthStartParams>,
) -> Response {
    let mut response = super::preferences::relay::<jeeves_protocol::ConnectorOAuthStartResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_OAUTH_START,
        serde_json::to_value(params).expect("OAuth params serialize"),
    )
    .await;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

pub async fn capability_probe(
    Extension(app): Extension<Arc<crate::AppServer>>,
    Json(params): Json<jeeves_protocol::CapabilityProbeParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::CapabilityProbeResponse>(
        &app,
        jeeves_protocol::methods::CAPABILITY_PROBE,
        serde_json::json!(params),
    )
    .await
}

fn refused(why: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": {"code": -32602, "message": format!("{why:#}")}})),
    )
        .into_response()
}

pub async fn lark_get(Extension(gateways): Extension<Arc<Gateways>>) -> Response {
    Json(gateways.status()).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LarkSave {
    pub config: LarkConfig,
    /// Only sent when it changes; the secret is never read back.
    pub app_secret: Option<String>,
}

/// Save, then (re)start or stop to match `enabled` — one click, one visible outcome.
pub async fn lark_save(
    Extension(gateways): Extension<Arc<Gateways>>,
    Json(body): Json<LarkSave>,
) -> Response {
    let enabled = body.config.enabled;
    if let Err(why) = gateways.save(body.config, body.app_secret.as_deref()) {
        return refused(why);
    }
    if enabled {
        if let Err(why) = gateways.start().await {
            return refused(why);
        }
    } else {
        gateways.stop().await;
    }
    Json(gateways.status()).into_response()
}

#[derive(Deserialize)]
pub struct Claim {
    #[serde(rename = "openId")]
    pub open_id: String,
}

pub async fn lark_claim(
    Extension(gateways): Extension<Arc<Gateways>>,
    Json(body): Json<Claim>,
) -> Response {
    match gateways.claim(&body.open_id).await {
        Ok(()) => Json(gateways.status()).into_response(),
        Err(why) => refused(why),
    }
}

pub async fn catalog(Extension(app): Extension<Arc<crate::AppServer>>) -> Response {
    super::preferences::relay::<jeeves_protocol::ConnectorCatalogResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_CATALOG,
        serde_json::json!({}),
    )
    .await
}

pub async fn tools(
    Extension(app): Extension<Arc<crate::AppServer>>,
    axum::extract::Query(p): axum::extract::Query<jeeves_protocol::CapabilityProbeParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::ConnectorToolsResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_TOOLS,
        serde_json::json!(p),
    )
    .await
}
pub async fn set_tool(
    Extension(app): Extension<Arc<crate::AppServer>>,
    Json(p): Json<jeeves_protocol::ConnectorToolsSetParams>,
) -> Response {
    super::preferences::relay::<jeeves_protocol::ConnectorToolsResponse>(
        &app,
        jeeves_protocol::methods::CONNECTOR_TOOLS_SET,
        serde_json::json!(p),
    )
    .await
}
