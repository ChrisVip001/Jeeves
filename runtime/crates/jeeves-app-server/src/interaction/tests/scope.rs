use super::*;

#[tokio::test]
async fn public_chat_methods_cannot_create_or_address_a_second_conversation() {
    let (_home, server, model) = setup().await;
    for (method, params) in [
        ("session/new", json!({"sessionId":"second"})),
        (
            "session/fork",
            json!({"sessionId":MAIN_SESSION,"newSessionId":"second"}),
        ),
        (
            "turn/start",
            json!({"sessionId":"second","message":"hello"}),
        ),
        (
            "turn/start",
            json!({"sessionId":"task-work:spoof","message":"hello","requestId":"outside"}),
        ),
    ] {
        let (sink, mut frames) = EventSink::channel();
        crate::handle(
            &server,
            JsonRpcRequest {
                jsonrpc: JSONRPC_VERSION.into(),
                id: RequestId::Number(1),
                method: method.into(),
                params: Some(params),
            },
            sink,
        )
        .await;
        assert!(
            matches!(
                frames.try_recv(),
                Ok(Frame::Response(JsonRpcMessage::Error(_)))
            ),
            "{method}"
        );
    }
    assert!(model.calls.lock().unwrap().is_empty());
    let sessions = server.chat.sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, MAIN_SESSION);
}

#[tokio::test]
async fn missing_session_points_to_the_main_conversation() {
    let (_home, server, _model) = setup().await;
    let (sink, mut frames) = EventSink::channel();
    crate::handle(
        &server,
        JsonRpcRequest {
            jsonrpc: JSONRPC_VERSION.into(),
            id: RequestId::Number(1),
            method: jeeves_protocol::methods::SESSION_GET.into(),
            params: Some(json!({"sessionId": "missing"})),
        },
        sink,
    )
    .await;
    let Frame::Response(JsonRpcMessage::Error(response)) = frames.try_recv().unwrap() else {
        panic!("expected a session lookup error");
    };
    assert_eq!(response.error.code, jeeves_protocol::codes::INVALID_PARAMS);
    assert!(response.error.message.contains("missing"));
    assert!(
        response.error.message.contains("session/main"),
        "{response:?}"
    );
}

#[tokio::test]
async fn task_handoff_distinguishes_past_execution_failures_from_current_capability() {
    let (_home, server, model) = setup().await;
    server
        .chat
        .append_exchange(
            "Run ls",
            "sandbox-exec exited with code 71",
            MAIN_SESSION,
            "app-server",
            None,
        )
        .await
        .unwrap();
    send(&server, "归纳会议议题", "handoff-after-old-failure", None)
        .await
        .unwrap();
    let calls = model.calls.lock().unwrap();
    let first = &calls[0];
    assert!(first
        .messages
        .iter()
        .any(|message| message.text().contains("code 71")));
    assert!(first
        .system_text()
        .unwrap()
        .contains("Historical tool failures are past observations"));
}

#[tokio::test]
async fn dropping_a_main_interaction_releases_its_handoff_tool() {
    let (_home, server, model) = setup().await;
    let (sink, mut frames) = EventSink::channel();
    let lock = server.session_write_lock(MAIN_SESSION);
    {
        let _pause_history = lock.lock().await;
        let interaction = start(
            &server,
            InteractionStartParams {
                session_id: MAIN_SESSION.into(),
                message_id: "first".into(),
                message: "Hello".into(),
                task_id: None,
                cwd: None,
                attachments: None,
                source: None,
            },
            RequestId::Number(1),
            &sink,
        );
        tokio::select! {
            result = interaction => panic!("interaction finished before history was loaded: {result:?}"),
            () = async {
                while let Some(frame) = frames.recv().await {
                    if matches!(frame, Frame::Notification(ServerNotification::TurnStarted(_))) {
                        return;
                    }
                }
                panic!("interaction closed before its turn started");
            } => (),
            () = tokio::time::sleep(Duration::from_secs(5)) => panic!("turn never started"),
        }
    }
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            session_id: Some(MAIN_SESSION.into()),
            message: "Good morning".into(),
            ..Default::default()
        },
        RequestId::Number(2),
        &sink,
    )
    .await
    .unwrap();
    let calls = model.calls.lock().unwrap();
    let request = calls.last().unwrap();
    assert!(!request.tools.iter().any(|tool| tool.name == "task"));
}

#[tokio::test]
async fn external_message_uses_main_without_disclosing_owner_history_or_tools() {
    let (_home, server, model) = setup().await;
    server
        .chat
        .append_exchange(
            "PRIVATE_OWNER_HISTORY",
            "only visible in the owner chat",
            MAIN_SESSION,
            "app-server",
            None,
        )
        .await
        .unwrap();
    let params = InteractionStartParams {
        session_id: MAIN_SESSION.into(),
        message_id: "channel-origin".into(),
        message: "What did we discuss?".into(),
        cwd: None,
        attachments: None,
        task_id: None,
        source: Some(jeeves_protocol::InteractionSource {
            channel: "lark".into(),
            chat_id: "channel-chat".into(),
            sender_id: "guest".into(),
            message_id: "chat-message".into(),
        }),
    };
    for id in [1, 2] {
        let (sink, _frames) = EventSink::channel();
        start(&server, params.clone(), RequestId::Number(id), &sink)
            .await
            .unwrap();
    }
    assert_eq!(server.chat.sessions().await.unwrap().len(), 1);
    let calls = model.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert!(!calls[0]
        .messages
        .iter()
        .any(|message| message.text().contains("PRIVATE_OWNER_HISTORY")));
    assert!(calls[0].tools.is_empty());
}
