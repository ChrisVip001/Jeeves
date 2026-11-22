//! Auth tests for the HTTP face: token three states, binding validation, constant-time compare.
//!
//! Hits `router()` directly (serve with a real listener is integration smoke territory);
//! the app-server uses the same fake as client_tests — an auto-responder answers `{}`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use tower::ServiceExt;

pub(crate) use crate::client::client_tests::fake;
use crate::{constant_time_eq, router, Gate};

#[tokio::test]
async fn health_probe_sends_complete_capability_parameters() {
    let (app, mut fake) = fake();
    let router = router(Gate::single(Arc::new(app), "fixture".into()));
    let probe = tokio::spawn(status_of(router, "/healthz", None));
    let request = fake.next_request().await;
    assert_eq!(request["method"], "session/capabilities");
    let valid: Result<jeeves_protocol::SessionCapabilitiesParams, _> =
        serde_json::from_value(request["params"].clone());
    if valid.is_ok() {
        fake.respond(
            &request["id"],
            json!({"protocolVersion":2,"accepted":[],"unsupported":[]}),
        )
        .await;
    } else {
        fake.fail(&request["id"], -32602, "missing capability parameters")
            .await;
    }
    assert_eq!(probe.await.unwrap(), StatusCode::OK);
}

#[tokio::test]
async fn capability_routes_relay_protocol_and_require_authentication() {
    let (app, mut fake) = fake();
    let router = router(Gate::single(Arc::new(app), "token".into()));
    assert_eq!(
        status_of(router.clone(), "/api/capability", None).await,
        StatusCode::UNAUTHORIZED
    );
    let list = tokio::spawn(
        router.clone().oneshot(
            Request::builder()
                .uri("/api/capability")
                .header("authorization", "Bearer token")
                .body(Body::empty())
                .unwrap(),
        ),
    );
    let request = fake.next_request().await;
    assert_eq!(request["method"], "capability/list");
    fake.respond(&request["id"], json!({"cards":[]})).await;
    assert_eq!(list.await.unwrap().unwrap().status(), StatusCode::OK);
    let probe = tokio::spawn(
        router.oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/capability/probe")
                .header("authorization", "Bearer token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"id":"mcp:fixture"}"#))
                .unwrap(),
        ),
    );
    let request = fake.next_request().await;
    assert_eq!(request["method"], "capability/probe");
    assert_eq!(request["params"]["id"], "mcp:fixture");
    fake.fail(&request["id"], -32602, "unknown capability")
        .await;
    assert_eq!(
        probe.await.unwrap().unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn new_connector_and_checkpoint_routes_are_human_authenticated_relays() {
    let (app, mut fake) = fake();
    let router = router(Gate::single(Arc::new(app), "token".into()));
    assert_eq!(
        status_of(router.clone(), "/api/connector/settings", None).await,
        StatusCode::UNAUTHORIZED
    );
    for (path, method, body, result) in [
        (
            "/api/connector/settings/save",
            "connector/settings/save",
            json!({"calendar":{"url":"https://example.test/cal","username":null,"credentialSource":null,"timezone":"UTC"}}),
            json!({"calendar":null,"email":null}),
        ),
        (
            "/api/connector/oauth/start",
            "connector/oauth/start",
            json!({"id":"mcp:fixture"}),
            json!({"authorizationUrl":"https://example.test/authorize"}),
        ),
        (
            "/api/task/git-checkpoint",
            "task/gitCheckpoint",
            json!({"taskId":"task","paths":["a.txt"],"confirmed":true}),
            json!({"taskId":"task","commit":"abc","branch":"jeeves/task"}),
        ),
    ] {
        let request = |auth: bool| {
            let mut builder = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json");
            if auth {
                builder = builder.header("authorization", "Bearer token");
            }
            builder.body(Body::from(body.to_string())).unwrap()
        };
        assert_eq!(
            router
                .clone()
                .oneshot(request(false))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let sent = tokio::spawn(router.clone().oneshot(request(true)));
        let rpc = fake.next_request().await;
        assert_eq!(rpc["method"], method);
        fake.respond(&rpc["id"], result).await;
        assert_eq!(sent.await.unwrap().unwrap().status(), StatusCode::OK);
    }
}

pub(crate) fn auto_respond(fake: crate::client::client_tests::Fake) {
    tokio::spawn(async move {
        let mut fake = fake;
        loop {
            let request = fake.next_request().await;
            let method = request["method"].as_str().unwrap_or_default().to_string();
            let id = request["id"].clone();
            let result = match method.as_str() {
                "config/read" => json!({"config": {
                    "provider": "anthropic", "model": "", "smallModel": "",
                    "maxTokens": 4096, "historyTurns": 10,
                    "consolidateEvery": 5, "retrievalTopK": 5,
                    "appleCalendar": false, "googleCalendar": false,
                    "experimental": false, "graphWorkflows": false, "home": "/tmp",
                }}),
                _ => json!({"data": []}),
            };
            fake.respond(&id, result).await;
        }
    });
}

pub(crate) async fn status_of(router: axum::Router, path: &str, auth: Option<&str>) -> StatusCode {
    let mut builder = Request::builder().uri(path);
    if let Some(token) = auth {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = router
        .oneshot(builder.body(Body::empty()).expect("the request body"))
        .await
        .expect("the routed response");
    response.status()
}

#[tokio::test]
async fn a_token_gates_the_api() {
    let (app, fake) = fake();
    auto_respond(fake);
    let router = router(Gate::single(Arc::new(app), "s3cret".to_string()));

    assert_eq!(
        status_of(router.clone(), "/api/data", None).await,
        StatusCode::UNAUTHORIZED,
        "no token means no pass"
    );
    assert_eq!(
        status_of(router.clone(), "/api/data", Some("wrong")).await,
        StatusCode::UNAUTHORIZED,
        "a wrong token means no pass"
    );
    assert_eq!(
        status_of(router.clone(), "/api/data", Some("s3cret")).await,
        StatusCode::OK,
        "the right token has to pass"
    );
    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/data")
                .header("authorization", "s3cret")
                .body(Body::empty())
                .expect("the request body"),
        )
        .await
        .expect("the routed response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn interrupt_answers_with_the_turns_own_fate() {
    let (app, fake) = fake();
    auto_respond(fake);
    let router = router(Gate::single(Arc::new(app), "fixture".into()));

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/interrupt")
                .header("authorization", "Bearer fixture")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"turnId":"no-such-turn"}"#))
                .expect("the request body"),
        )
        .await
        .expect("the routed response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn the_runtime_does_not_serve_the_workspace_ui() {
    let (app, _fake) = fake();
    let router = router(Gate::single(Arc::new(app), "fixture".into()));
    for path in ["/", "/main.js", "/account/", "/app/u-000000000000/"] {
        assert_eq!(
            status_of(router.clone(), path, Some("fixture")).await,
            StatusCode::NOT_FOUND
        );
    }
}

#[test]
fn constant_time_comparison_agrees_with_equality() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"abcd"));
    assert!(constant_time_eq(b"", b""));
}
