//! Events tests: the ping mapping, the events channel's auth gate and content
//! type, and that no other origin is let in.
//!
//! A full "two clients see the same ping" run needs a live turn and a held-open SSE
//! body — that is smoke territory (smoke.sh drives the real server); here we pin the
//! pure parts and the headers.

use super::api::EventPing;
use super::router_tests::{auto_respond, fake, status_of};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use jeeves_protocol::{ServerNotification, TurnStartedNotification};
use std::sync::Arc;
use tower::ServiceExt;

fn router_with_token() -> Router {
    let (app, fake) = fake();
    auto_respond(fake);
    super::router(super::Gate::single(Arc::new(app), "s3cret".to_string()))
}

#[test]
fn pings_distill_the_notifications_that_matter() {
    let started = ServerNotification::TurnStarted(TurnStartedNotification {
        turn_id: "t-1".to_string(),
        session_id: "default".to_string(),
        user_message: "hello".to_string(),
        ts: "2026-11-05T00:00:00.000Z".to_string(),
    });
    let ping = EventPing::of(&started).expect("a turn start is one ping");
    assert_eq!(ping.kind, "turn_started");
    assert_eq!(ping.session_id.as_deref(), Some("default"));

    let noise = ServerNotification::TextDelta(jeeves_protocol::TextDeltaNotification {
        turn_id: "t-1".to_string(),
        delta: "one".to_string(),
    });
    assert!(
        EventPing::of(&noise).is_none(),
        "a token-by-token stream does not go through the gate"
    );
}

#[tokio::test]
async fn the_events_channel_is_guarded_and_sse() {
    let router = router_with_token();

    assert_eq!(
        status_of(router.clone(), "/api/events", None).await,
        StatusCode::UNAUTHORIZED,
        "the event stream needs a token like any other API"
    );

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/events")
                .header("authorization", "Bearer s3cret")
                .body(Body::empty())
                .expect("the request body"),
        )
        .await
        .expect("the routed response");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|value| value
                .to_str()
                .unwrap_or("")
                .starts_with("text/event-stream")),
        "the content-type has to be SSE"
    );
}

#[tokio::test]
async fn persistent_sse_bodies_end_on_shutdown_even_if_the_client_keeps_them_open() {
    let (app, _fake) = fake();
    let app = Arc::new(app);
    let router = super::router(super::Gate::single(app.clone(), "fixture".into()));
    let mut bodies = Vec::new();
    for path in ["/api/events", "/api/notifications"] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("authorization", "Bearer fixture")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        bodies.push(response.into_body());
    }
    app.begin_shutdown();
    for body in bodies {
        let bytes = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            axum::body::to_bytes(body, 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(bytes.is_empty());
    }
}

#[tokio::test]
async fn no_cross_origin_access_to_the_api() {
    let router = router_with_token();
    let response = router
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/data")
                .header(header::ORIGIN, "https://elsewhere.example")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                .body(Body::empty())
                .expect("the request body"),
        )
        .await
        .expect("the routed response");
    assert!(response
        .headers()
        .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .is_none());
}

#[test]
fn task_pings_distill_the_checkpoint_lifecycle() {
    let checkpointed =
        ServerNotification::TurnCheckpointed(jeeves_protocol::TurnCheckpointedNotification {
            turn_id: "t-crash".to_string(),
            session_id: "default".to_string(),
            iterations: 3,
        });
    let ping = EventPing::of(&checkpointed).expect("the edge reaching the disk is one ping");
    assert_eq!(ping.kind, "task_progress");
    assert_eq!(ping.turn_id.as_deref(), Some("t-crash"));

    let resumed = ServerNotification::TurnResumed(jeeves_protocol::TurnResumedNotification {
        turn_id: "t-crash".to_string(),
        session_id: "default".to_string(),
        from_iterations: 3,
        reason: "reconcile: crash recovery".to_string(),
    });
    let ping = EventPing::of(&resumed).expect("a resume is one ping");
    assert_eq!(ping.kind, "task_resumed");
    assert_eq!(ping.session_id.as_deref(), Some("default"));

    let suspended = ServerNotification::TurnSuspended(jeeves_protocol::TurnSuspendedNotification {
        turn_id: "t-crash".to_string(),
        session_id: "default".to_string(),
        reason: "interrupted by user".to_string(),
    });
    let ping = EventPing::of(&suspended).expect("a suspension is one ping");
    assert_eq!(ping.kind, "task_suspended");
}
