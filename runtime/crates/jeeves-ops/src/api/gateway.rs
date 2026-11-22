//! Gateway requests reuse the cockpit's existing app-server and authentication.

use crate::AppServer;
use axum::{extract::Extension, Json};
use jeeves_protocol::{codes, methods, JsonRpcRequest};
use serde_json::{json, Value};
use std::sync::Arc;

fn allowed(method: &str) -> bool {
    [
        methods::INTERACTION_START,
        methods::SESSION_MAIN,
        methods::TASK_GET,
        methods::TASK_WAKE,
        methods::APPROVAL_PENDING,
        methods::APPROVAL_RESPOND,
    ]
    .contains(&method)
}

pub async fn rpc(
    Extension(app): Extension<Arc<AppServer>>,
    Json(request): Json<JsonRpcRequest>,
) -> Json<Value> {
    let id = request.id;
    if !allowed(&request.method) {
        return Json(
            json!({"jsonrpc":"2.0", "id":id, "error":{"code":codes::METHOD_NOT_FOUND,"message":"Method is not available to chat gateways"}}),
        );
    }
    if request.method == methods::INTERACTION_START
        && !request
            .params
            .as_ref()
            .and_then(|p| p.get("source"))
            .is_some_and(Value::is_object)
    {
        return Json(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":codes::INVALID_PARAMS,"message":"A gateway message must include its channel source"}}),
        );
    }
    Json(
        match app
            .call(&request.method, request.params.unwrap_or(Value::Null))
            .await
        {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_surface_excludes_runtime_configuration_and_task_execution() {
        assert!(!allowed(methods::TURN_START));
        assert!(allowed(methods::INTERACTION_START));
        assert!(allowed(methods::TASK_WAKE));
        for method in [
            "config/write",
            "task/start",
            "task/create",
            "memory/remember",
            methods::RESPONSIBILITY_PREPARE,
            methods::RESPONSIBILITY_REGISTER,
            methods::RESPONSIBILITY_SET_PAUSED,
            methods::DEVICE_PAIR,
            methods::DEVICE_REVOKE,
            methods::DEVICE_LIST,
            methods::DEVICE_REPORT,
            "unknown",
        ] {
            assert!(!allowed(method));
        }
    }
}
