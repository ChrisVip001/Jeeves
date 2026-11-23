//! Settings → Credentials: names the owner stored for commands to *use*. Values are write-only.

use super::refusal;
use crate::AppServer;
use axum::{Json, extract::Extension, response::IntoResponse, response::Response};
use jeeves_config::{models::SecretStore, vault};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub async fn list(Extension(app): Extension<Arc<AppServer>>) -> Response {
    names_response(&app, None)
}

fn names_response(app: &AppServer, stored_in: Option<&str>) -> Response {
    match vault::read_names(app.home()) {
        Ok(names) => {
            let mut body = json!({ "names": names });
            if let Some(source) = stored_in {
                body["storedIn"] = json!(source);
            }
            Json(body).into_response()
        }
        Err(why) => refusal(why),
    }
}

#[derive(Deserialize)]
pub struct Put {
    pub name: String,
    pub value: String,
}

pub async fn put(Extension(app): Extension<Arc<AppServer>>, Json(body): Json<Put>) -> Response {
    let name = body.name.trim().to_ascii_uppercase();
    match vault::put(&mut SecretStore::auto(), app.home(), &name, &body.value) {
        Ok(landed) => names_response(&app, Some(landed)),
        Err(why) => refusal(why),
    }
}

#[derive(Deserialize)]
pub struct Remove {
    pub name: String,
}

pub async fn remove(
    Extension(app): Extension<Arc<AppServer>>,
    Json(body): Json<Remove>,
) -> Response {
    match vault::remove(&mut SecretStore::auto(), app.home(), &body.name) {
        Ok(()) => names_response(&app, None),
        Err(why) => refusal(why),
    }
}
