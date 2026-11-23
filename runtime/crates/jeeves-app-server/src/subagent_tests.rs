//! The subagent's two hardening measures, run end to end:
//! structured results (validation + one retry + fallback) and the "the report is a retelling" wrapping.

use std::sync::Arc;

use jeeves_protocol::{RequestId, ServerNotification, TurnStartParams};
use jeeves_provider::Resolved;
use jeeves_provider::mock::Mock;
use serde_json::json;

use crate::{EventSink, Frame, run_turn};

async fn server_with(
    responses: Vec<jeeves_provider::CreateResponse>,
) -> (crate::Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        provider: "openai".to_string(),
        api_key: Some("test-key".to_string()),
        model: Some("test-model".to_string()),
        small_model: Some("test-model".to_string()),
        delegate_enabled: true,
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    server.install_provider(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(Mock::new(responses)),
        model: "test-model".to_string(),
        small_model: "test-model".to_string(),
    });
    (server, dir)
}

async fn collect(server: &crate::Server, message: &str) -> (String, String) {
    let (sink, mut rx) = EventSink::channel();
    run_turn(
        server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: message.to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .expect("this turn should finish");
    drop(sink);

    let mut tool_output = String::new();
    let mut reply = String::new();
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Notification(notification) = frame {
            match notification {
                ServerNotification::ToolCompleted(done) => tool_output = done.output.clone(),
                ServerNotification::TurnCompleted(done) => reply = done.reply.clone(),
                _ => {}
            }
        }
    }
    (tool_output, reply)
}

#[tokio::test]
async fn a_structured_result_is_validated_and_retried_once() {
    let (server, _dir) = server_with(vec![
        Mock::tool_use(
            "call-1",
            "delegate_task",
            json!({
                "task": "work out 2+2",
                "result_schema": {
                    "type": "object",
                    "required": ["answer"],
                    "properties": {"answer": {"type": "integer"}}
                }
            }),
        ),
        Mock::text("I think it is four."),
        Mock::text(r#"{"answer": 4}"#),
        Mock::text("the subagent says the answer is 4."),
    ])
    .await;

    let (tool_output, reply) = collect(&server, "send a subagent to work out 2+2").await;

    assert!(reply.contains("4"), "{reply}");
    assert!(
        tool_output.contains("reported"),
        "the report has to mark it as reported speech: {tool_output}"
    );
    assert!(
        tool_output.contains(r#"{"answer":4}"#),
        "once compliant, what comes back should be compact JSON: {tool_output}"
    );
    assert!(
        !tool_output.contains("I think it is four"),
        "the non-compliant version must not stay in the conclusion: {tool_output}"
    );
}

#[tokio::test]
async fn a_structured_result_falls_back_to_prose_after_two_failures() {
    let (server, _dir) = server_with(vec![
        Mock::tool_use(
            "call-1",
            "delegate_task",
            json!({
                "task": "work out 2+2",
                "result_schema": {
                    "type": "object",
                    "required": ["answer"],
                    "properties": {"answer": {"type": "integer"}}
                }
            }),
        ),
        Mock::text("four."),
        Mock::text("four again."),
        Mock::text("it says four."),
    ])
    .await;

    let (tool_output, _) = collect(&server, "send a subagent to work out 2+2").await;

    assert!(
        tool_output.contains("failed validation twice"),
        "two non-compliant tries have to be reported plainly: {tool_output}"
    );
    assert!(
        tool_output.contains("four."),
        "on falling back, keep its own words (the work must not be wasted): {tool_output}"
    );
}
