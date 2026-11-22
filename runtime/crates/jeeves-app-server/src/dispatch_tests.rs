//! Tests for the outlet layer: rows in the db → values in the protocol.
//!
//! Only two pure functions are tested (`iso` and `to_message`). The part that really talks to state is
//! pinched from both ends — inside, the SQL in `state_tests.rs`; outside, the real HTTP round-trips of
//! `smoke-dashboard.sh`. This layer must never grow a second set of SQL.

use super::{iso, iso_opt, to_message};
use jeeves_protocol::MessageRole;
use jeeves_state::MessageRow;

fn row(role: &str, meta: Option<&str>) -> MessageRow {
    MessageRow {
        id: 7,
        source: "fixture".into(),
        task_id: None,
        receipt_status: None,
        approval_decisions: None,
        role: role.to_string(),
        content: "hello".to_string(),
        at: "2026-09-19 16:46:49".to_string(),
        meta: meta.map(str::to_string),
    }
}

#[test]
fn sqlite_datetime_goes_out_as_iso_8601_utc() {
    assert_eq!(
        iso("2026-09-19 16:46:49".to_string()),
        "2026-09-19T16:46:49Z"
    );
    assert_eq!(iso_opt(None), None);
    assert_eq!(
        iso_opt(Some("2026-01-02 03:04:05".to_string())),
        Some("2026-01-02T03:04:05Z".to_string())
    );
}

#[test]
fn iso_passes_through_what_it_cannot_parse() {
    for weird in ["", "2026-09-19T16:46:49+08:00", "not a time"] {
        assert_eq!(iso(weird.to_string()), weird);
    }
}

#[test]
fn only_renderable_roles_make_it_into_the_protocol() {
    assert_eq!(
        to_message(row("user", None)).expect("user").role,
        MessageRole::User
    );
    assert_eq!(
        to_message(row("assistant", None)).expect("assistant").role,
        MessageRole::Assistant
    );
    assert!(to_message(row("system", None)).is_none());
    assert!(to_message(row("", None)).is_none());
}

#[test]
fn broken_telemetry_does_not_take_the_message_with_it() {
    let raw = r#"{"gate":null,"graph":null,"iterations":2,"latencyMs":120,
                  "tools":[],"model":"qwen","provider":"dashscope"}"#;
    let meta = to_message(row("assistant", Some(raw)))
        .expect("assistant")
        .meta
        .expect("this telemetry should parse");
    assert_eq!(meta.model, "qwen");
    assert_eq!(meta.iterations, 2);
    assert_eq!(meta.provider, "dashscope");

    let broken = to_message(row("assistant", Some("this is not JSON"))).expect("assistant");
    assert!(broken.meta.is_none());
    assert_eq!(broken.content, "hello");
}

#[test]
fn id_and_timestamp_survive_the_trip() {
    let message = to_message(row("user", None)).expect("user");
    assert_eq!(message.id, 7);
    assert_eq!(message.at, "2026-09-19T16:46:49Z");
    assert!(message.meta.is_none());
}

#[tokio::test]
async fn human_memory_validity_and_provenance_round_trip() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = crate::Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            provider: "test-unconfigured".into(),
            ..Default::default()
        },
    )
    .await;
    let (row, _) = server
        .facts
        .add_consolidated_from("alex", "likes tea", "user", &["chat:1@digest".into()])
        .await
        .unwrap()
        .unwrap();
    let (sink, _) = crate::EventSink::channel();
    let id = jeeves_protocol::RequestId::Number(1);
    let params =
        serde_json::json!({"factId":row.id,"validFrom":null,"validUntil":"2000-01-01T00:00:00Z"});
    let result = super::dispatch(
        &server,
        jeeves_protocol::methods::MEMORY_VALIDITY,
        Some(&params),
        &id,
        &sink,
    )
    .await
    .unwrap();
    let super::Outcome::Value(value) = result else {
        panic!("Expected response");
    };
    assert_eq!(value["changed"], true);
    assert!(server.facts.search("tea", 10).await.unwrap().is_empty());
    let params = serde_json::json!({"factId":row.id});
    let result = super::dispatch(
        &server,
        jeeves_protocol::methods::MEMORY_PROVENANCE,
        Some(&params),
        &id,
        &sink,
    )
    .await
    .unwrap();
    let super::Outcome::Value(value) = result else {
        panic!("Expected response");
    };
    assert_eq!(value["observations"][0]["sourceRefs"][0], "chat:1@digest");
    assert_eq!(value["validUntil"], "2000-01-01T00:00:00+00:00");
    assert_eq!(value["validityOrigin"], "human");
    server
        .chat
        .append_exchange("I feel tired today.", "I hear you.", "chat", "test", None)
        .await
        .unwrap();
    let digest = jeeves_state::ledger::digest_of(
        serde_json::to_string(&("user", "I feel tired today."))
            .unwrap()
            .as_bytes(),
    );
    let observation = jeeves_state::facts::TransientObservation {
        source_ref: format!("chat:1@{digest}"),
        quote: "feel tired today".into(),
        confidence: 0.7,
        ttl_hours: 3,
    };
    let (temporary, _) = server
        .facts
        .add_transient_from(
            "user",
            "Feels tired today",
            "user",
            std::slice::from_ref(&observation.source_ref),
            &observation,
        )
        .await
        .unwrap()
        .unwrap();
    let params = serde_json::json!({"factId": temporary.id});
    let result = super::dispatch(
        &server,
        jeeves_protocol::methods::MEMORY_PROVENANCE,
        Some(&params),
        &id,
        &sink,
    )
    .await
    .unwrap();
    let super::Outcome::Value(value) = result else {
        panic!("Expected response");
    };
    assert_eq!(value["validityOrigin"], "consolidation");
    let assessment: serde_json::Value =
        serde_json::from_str(value["observations"][0]["assessmentJson"].as_str().unwrap()).unwrap();
    assert_eq!(assessment["sourceRef"], observation.source_ref);
    assert_eq!(assessment["confidence"], 0.7);
    assert_eq!(assessment["quote"], "feel tired today");
}
