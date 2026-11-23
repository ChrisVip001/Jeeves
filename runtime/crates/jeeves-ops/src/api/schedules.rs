//! Schedules HTTP endpoints: thin proxies over the `schedule/*` methods. The Runtime owns the job
//! table, the cron math and the validation; this layer only carries the answer to the panel.

use axum::Json;
use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use std::sync::Arc;

use super::{failed, outcome, refusal};
use crate::{AppServer, capabilities};
use jeeves_protocol::{ScheduleChangeResponse, ScheduleListResponse, methods};
use serde_json::{Value, json};

/// `GET /api/schedules/jobs` — every declared job with its runtime state and next run.
pub async fn schedule_jobs(Extension(app): Extension<Arc<AppServer>>) -> Response {
    match app
        .request::<ScheduleListResponse>(methods::SCHEDULE_LIST, json!({}))
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(error) => failed(StatusCode::OK, error),
    }
}

/// `POST /api/schedules/save` — add or replace one stored job.
pub async fn schedule_save(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<Value>,
) -> Response {
    match app
        .request::<ScheduleChangeResponse>(methods::SCHEDULE_SAVE, body)
        .await
    {
        Ok(done) => {
            eprintln!("(jeeves) capability hub: {}", done.message);
            outcome(true, done.message)
        }
        Err(error) => refusal(error.message),
    }
}

/// `POST /api/schedules/remove` — delete one stored job (skill jobs are refused).
pub async fn schedule_remove(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<capabilities::NameBody>,
) -> Response {
    match app
        .request::<ScheduleChangeResponse>(
            methods::SCHEDULE_REMOVE,
            json!({"name": body.name.trim()}),
        )
        .await
    {
        Ok(done) => {
            eprintln!("(jeeves) capability hub: {}", done.message);
            outcome(true, done.message)
        }
        Err(error) => refusal(error.message),
    }
}
