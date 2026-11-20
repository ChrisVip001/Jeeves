//! Wire-format assertions for the v2 protocol.
//!
//! These are **ordinary unit tests** (no `#[ignore]`); `cargo test` runs them.
//! export_tests owns artifact correctness; this file owns "what the serialized bytes look like" —
//! i.e. what the frontend and the Python SDK actually receive.

use super::v2::*;

#[test]
fn notification_is_tagged_with_type() {
    let n = ServerNotification::TurnStarted(TurnStartedNotification {
        turn_id: "t-1".to_string(),
        session_id: "s-1".to_string(),
        user_message: "hi".to_string(),
        ts: "2026-01-01T00:00:00+00:00".to_string(),
    });
    let v: serde_json::Value = serde_json::to_value(&n).expect("serialise");
    assert_eq!(v["type"], "turnStarted");
    assert_eq!(v["turnId"], "t-1");
}

#[test]
fn every_notification_variant_has_a_stable_tag() {
    let cases: Vec<(ServerNotification, &str)> = vec![
        (
            ServerNotification::TurnStarted(TurnStartedNotification {
                turn_id: "t".into(),
                session_id: "default".into(),
                user_message: "hi".into(),
                ts: "2026-09-19T00:00:00.000Z".into(),
            }),
            "turnStarted",
        ),
        (
            ServerNotification::TextDelta(TextDeltaNotification {
                turn_id: "t".into(),
                delta: "he".into(),
            }),
            "textDelta",
        ),
        (
            ServerNotification::ConsolidationCompleted(ConsolidationCompletedNotification {
                new_facts: 2,
            }),
            "consolidationCompleted",
        ),
        (
            ServerNotification::Error(ErrorNotification {
                code: -32000,
                message: "boom".into(),
                data: None,
            }),
            "error",
        ),
    ];

    for (n, expected) in cases {
        let v: serde_json::Value = serde_json::to_value(&n).expect("serialise");
        assert_eq!(v["type"], expected, "{expected}'s discriminant changed");
    }
}

#[test]
fn params_are_camel_case_on_the_wire() {
    let p = TurnStartParams {
        session_id: None,
        request_id: None,
        message: "hi".into(),
        stream: Some(true),
        cwd: None,
        attachments: None,
        tier: None,
    };
    let v: serde_json::Value = serde_json::to_value(&p).expect("serialise");
    assert!(v.as_object().expect("object").contains_key("sessionId"));
    assert!(v["sessionId"].is_null());
    assert_eq!(v["message"], "hi");
    assert_eq!(v["stream"], true);
    assert!(!v.as_object().unwrap().contains_key("session_id"));
}

#[test]
fn tool_status_round_trips() {
    for (status, wire) in [(ToolStatus::Ok, "ok"), (ToolStatus::Error, "error")] {
        let v: serde_json::Value = serde_json::to_value(status).expect("serialise");
        assert_eq!(v, wire);
        let back: ToolStatus = serde_json::from_value(v).expect("deserialise");
        assert_eq!(back, status);
    }
}

#[test]
fn message_role_and_page_cursor_are_camel_case_on_the_wire() {
    for (role, wire) in [
        (MessageRole::User, "user"),
        (MessageRole::Assistant, "assistant"),
    ] {
        let v: serde_json::Value = serde_json::to_value(role).expect("serialise");
        assert_eq!(v, wire);
    }

    let p = SessionMessagesParams {
        session_id: "default".into(),
        cursor: Some("42".into()),
        limit: None,
    };
    let v: serde_json::Value = serde_json::to_value(&p).expect("serialise");
    assert_eq!(v["sessionId"], "default");
    assert_eq!(v["cursor"], "42");
    assert!(v.as_object().unwrap().contains_key("limit"));

    let r = SessionMessagesResponse {
        active_inputs: None,
        data: vec![],
        next_cursor: None,
    };
    let v: serde_json::Value = serde_json::to_value(&r).expect("serialise");
    assert_eq!(v["nextCursor"], serde_json::Value::Null);
}

#[test]
fn settings_patch_default_leaves_everything_unset() {
    let v: serde_json::Value = serde_json::to_value(SettingsPatch::default()).expect("serialise");
    let obj = v.as_object().expect("object");
    assert!(
        !obj.is_empty(),
        "every patch field has to appear on the wire (as null)"
    );
    assert!(obj.values().all(|x| x.is_null()));
}
