//! HTTP route facade; resource modules translate the app-server protocol.

use crate::capabilities;
use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use jeeves_protocol::{ErrorObject, codes};

mod overview;
pub use overview::approvals;
pub use overview::data;
pub use overview::usage;
mod preferences;
pub use preferences::{
    config_get, config_set, model_catalog, permission_get, permission_set, remember,
};
mod sessions;
pub use sessions::approval;
pub use sessions::follow_up;
pub use sessions::interrupt;
pub use sessions::session;
pub use sessions::session_meta;
pub use sessions::session_update;
pub use sessions::steer;
pub use sessions::turns;
mod tasks;
pub use tasks::task;
pub use tasks::task_events;
pub use tasks::tasks;
pub use tasks::turn_operation;
pub use tasks::{task_git_checkpoint, task_integrate};
pub use tasks::{task_git_checkpoint_policy, task_git_checkpoint_policy_set};
pub(crate) mod responsibilities;
mod schedules;
pub use schedules::schedule_jobs;
pub use schedules::schedule_remove;
pub use schedules::schedule_save;
mod files;

pub use files::fs_browse;
pub use files::fs_mkdir;
pub use files::fs_recents;
pub use files::fs_recents_record;

pub use files::upload;
mod models;
pub use models::capability_inventory;
pub use models::mcp;
pub use models::mcp_remove;
pub use models::mcp_save;
pub use models::model_profiles;
pub use models::model_remove;
pub use models::model_routes;
pub use models::model_routes_save;
pub use models::model_save;
pub use models::skills;
pub use models::skills_install;
pub use models::skills_remove;
pub(crate) mod connections;
pub(crate) mod gateway;
pub(crate) mod inbox;
mod stream;
pub(crate) mod vault;

pub use stream::events;
pub use stream::run;
pub(crate) use stream::run_headless;
mod hooks;
pub use hooks::hook;

/// `GET /healthz` — is the Runtime answering? 200 with versions, or 503. A probe asks often,
/// so this is the cheapest round trip the protocol has (`session/capabilities`), bounded.
pub async fn healthz(axum::extract::State(gate): axum::extract::State<crate::Gate>) -> Response {
    let app = gate.app();
    let probe = app.probe();
    match tokio::time::timeout(std::time::Duration::from_secs(3), probe).await {
        Ok(Ok(answer)) => Json(serde_json::json!({
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "protocolVersion": answer.protocol_version,
        }))
        .into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "status": "unavailable" })),
        )
            .into_response(),
    }
}

/// On error, return the protocol's own `ErrorObject`, untouched.
///
/// The status code is not 200: on the `fetch` side "this trip failed" is obvious at a
/// glance, while the body is still the familiar `{code, message}` — the page can just show
/// the message; what app-server wrote was meant for users in the first place.
pub(crate) fn failed(status: StatusCode, error: ErrorObject) -> Response {
    (status, Json(error)).into_response()
}

/// A refusal the page can show verbatim: 200 + the protocol's `ErrorObject`, the same
/// convention as `/api/task` — a human action refused is an answer, not a fault.
fn refusal(message: String) -> Response {
    failed(
        StatusCode::OK,
        ErrorObject {
            code: codes::INVALID_PARAMS,
            message,
            data: None,
        },
    )
}

fn outcome(ok: bool, message: String) -> Response {
    Json(capabilities::Outcome { ok, message }).into_response()
}

#[cfg(test)]
use files::{create_subdir, list_dirs, safe_extension};
#[cfg(test)]
pub(crate) use stream::EventPing;
#[cfg(test)]
use stream::belongs;
#[cfg(test)]
#[path = "api_tests.rs"]
mod api_tests;

pub use inbox::inbox;
pub use preferences::main_session;
pub use stream::{interaction, notifications};

pub use preferences::pending_approvals;

pub(crate) mod browser;
pub(crate) mod desktop;
pub(crate) mod devices;
