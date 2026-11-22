//! Hooks HTTP endpoints.

use super::{failed, run_headless};
use crate::webhooks;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Json;
use jeeves_protocol::{codes, ErrorObject, TurnStartParams};
use serde_json::json;

/// `POST /api/hook/:name` — an inbound webhook runs one turn (see `webhooks.rs` for the fences).
///
/// Deliberately **outside** the dashboard token layer: a foreign machine proves itself with the
/// hook's own secret (`X-Jeeves-Secret`), not with a token it was never given.
pub async fn hook(
    State(gate): State<crate::Gate>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let app = gate.app();
    let config = match webhooks::load(app.home()) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "(jeeves) webhooks.json has a problem; inbound hooks are all disabled: {error:#}"
            );
            return failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorObject {
                    code: codes::INTERNAL_ERROR,
                    message: format!("webhooks.json has a problem: {error}"),
                    data: None,
                },
            );
        }
    };
    let Some(hook) = webhooks::find(&config, &name).filter(|hook| hook.enabled()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let credential = webhooks::Credential {
        secret: header(&headers, "x-jeeves-secret"),
        timestamp: header(&headers, "x-jeeves-timestamp"),
        signature: header(&headers, "x-jeeves-signature"),
    };
    if let Err(refusal) = webhooks::admit(
        gate.admissions(),
        hook,
        &credential,
        &body,
        chrono::Utc::now().timestamp(),
        std::time::Instant::now(),
    ) {
        return refusal_response(refusal);
    }

    let hook_name = hook.name.clone();
    let params = TurnStartParams {
        request_id: None,
        session_id: None,
        message: hook.render(&body),
        stream: Some(false),
        cwd: None,
        attachments: None,
        tier: hook.tier.clone(),
    };

    if let Some(callback) = hook.callback_url().map(str::to_string) {
        tokio::spawn(async move {
            let payload = match run_headless(&app, params).await {
                Ok(run) => json!({
                    "hook": hook_name,
                    "ok": true,
                    "result": run,
                }),
                Err(error) => json!({
                    "hook": hook_name,
                    "ok": false,
                    "error": error,
                }),
            };
            webhooks::deliver(&callback, payload).await;
        });
        return (StatusCode::ACCEPTED, Json(webhooks::accepted(&hook.name))).into_response();
    }

    match run_headless(&app, params).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}

/// One header as text; absent or non-UTF8 reads as empty (the policy treats empty as absent).
pub(super) fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

/// A refusal a script can read: the status is the contract, this sentence is for the operator.
pub(super) fn refuse(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

pub(super) fn refusal_response(refusal: webhooks::Refusal) -> Response {
    match refusal {
        webhooks::Refusal::Unauthorized(reason) => refuse(StatusCode::UNAUTHORIZED, reason),
        webhooks::Refusal::Replay => refuse(
            StatusCode::CONFLICT,
            "this signature has already been used (replay refused)",
        ),
        webhooks::Refusal::TooFast(wait) => {
            let mut response = refuse(
                StatusCode::TOO_MANY_REQUESTS,
                &format!("this hook's per-minute quota is spent; try again in {wait} seconds"),
            );
            if let Ok(value) = axum::http::HeaderValue::from_str(&wait.to_string()) {
                response.headers_mut().insert("retry-after", value);
            }
            response
        }
    }
}
