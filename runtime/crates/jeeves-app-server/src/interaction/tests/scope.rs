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
    assert!(
        first
            .messages
            .iter()
            .any(|message| message.text().contains("code 71"))
    );
    assert!(
        first
            .system_text()
            .unwrap()
            .contains("Historical tool failures are past observations")
    );
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
    assert!(
        !calls[0]
            .messages
            .iter()
            .any(|message| message.text().contains("PRIVATE_OWNER_HISTORY"))
    );
    assert!(calls[0].tools.is_empty());
}

#[tokio::test]
async fn one_handoff_commits_its_feishu_origin_and_only_that_sender_in_that_chat_can_continue() {
    let (_home, server, _) = setup_with_settings(jeeves_config::Settings {
        dispatchers: vec!["owner".into(), "colleague".into()],
        ..Default::default()
    })
    .await;
    let (sink, _) = EventSink::channel();
    let interaction =
        |key: &str, chat: &str, sender: &str, message: &str, task_id: Option<String>| {
            InteractionStartParams {
                session_id: MAIN_SESSION.into(),
                message_id: key.into(),
                message: message.into(),
                cwd: None,
                attachments: None,
                task_id,
                source: Some(InteractionSource {
                    channel: "lark".into(),
                    chat_id: chat.into(),
                    sender_id: sender.into(),
                    message_id: key.into(),
                }),
            }
        };
    let p = interaction("assign", "room-a", "owner", "归纳会议议题", None);
    start(&server, p.clone(), RequestId::Number(1), &sink)
        .await
        .unwrap();
    let task = server.tasks.list(3).await.unwrap().pop().unwrap();
    wait(&server, &task.id).await;
    let origin = jeeves_state::task_sources::get(&server.pool, &task.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            origin.chat_id.as_str(),
            origin.sender_id.as_str(),
            origin.message_id.as_str()
        ),
        ("room-a", "owner", "assign")
    );
    for (key, chat, sender) in [
        ("foreign-chat", "room-b", "owner"),
        ("foreign-sender", "room-a", "colleague"),
    ] {
        let update = interaction(key, chat, sender, "补充来源", Some(task.id.clone()));
        assert!(
            start(&server, update, RequestId::Number(2), &sink)
                .await
                .unwrap_err()
                .message
                .contains("unavailable in this channel")
        );
    }
    assert_eq!(server.runs.count_by_task(&task.id).await.unwrap(), 1);
    let update = interaction("mine", "room-a", "owner", "补充来源", Some(task.id.clone()));
    start(&server, update.clone(), RequestId::Number(3), &sink)
        .await
        .unwrap();
    wait(&server, &task.id).await;
    start(&server, update, RequestId::Number(4), &sink)
        .await
        .unwrap();
    assert_eq!(server.runs.count_by_task(&task.id).await.unwrap(), 2);
    let revisions: Vec<String> =
        sqlx::query_scalar("SELECT instruction FROM task_amendments WHERE task_id=?")
            .bind(&task.id)
            .fetch_all(&server.pool)
            .await
            .unwrap();
    assert_eq!(revisions, ["补充来源"]);
}

#[tokio::test]
async fn interrupted_handoff_replays_only_committed_work_and_a_rejected_enqueue_leaves_no_task() {
    let (_home, server, model) = setup().await;
    sqlx::query("CREATE TRIGGER block_dispatch BEFORE INSERT ON task_dispatch BEGIN SELECT RAISE(ABORT,'fixture rejects enqueue'); END")
        .execute(&server.pool).await.unwrap();
    let events = send(&server, "归纳会议议题", "blocked", None)
        .await
        .unwrap();
    assert!(events.iter().any(|e|matches!(e,ServerNotification::ToolCompleted(c) if c.output.contains("fixture rejects enqueue"))));
    assert!(server.tasks.list(5).await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER block_dispatch")
        .execute(&server.pool)
        .await
        .unwrap();
    send(&server, "归纳会议议题", "accepted", None)
        .await
        .unwrap();
    let task = server.tasks.list(1).await.unwrap().pop().unwrap();
    wait(&server, &task.id).await;
    let op: String =
        sqlx::query_scalar("SELECT operation_id FROM conversation_handoffs WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let operation = jeeves_state::operations::ToolOperations::new(server.pool.clone())
        .get(&op)
        .await
        .unwrap()
        .unwrap();
    let mut checkpoint = jeeves_state::Checkpoints::new(server.pool.clone())
        .get(&operation.turn_id)
        .await
        .unwrap()
        .unwrap();
    checkpoint.round_id = Some(operation.round_id.clone());
    sqlx::query("UPDATE tool_operations SET status='intent',output_digest=NULL WHERE id=?")
        .bind(op)
        .execute(&server.pool)
        .await
        .unwrap();
    let proposal = serde_json::from_str(&operation.input_json).unwrap();
    let proof = super::super::handoff::committed_result(
        &server,
        &checkpoint,
        &operation.tool_use_id,
        &proposal,
    )
    .await
    .unwrap()
    .unwrap();
    assert!(proof.contains(&task.id));
    let mut changed = proposal;
    changed["items"][0]["instruction"] = json!("different instruction");
    assert!(
        super::super::handoff::committed_result(
            &server,
            &checkpoint,
            &operation.tool_use_id,
            &changed
        )
        .await
        .unwrap()
        .is_none()
    );
    sqlx::query("UPDATE interaction_receipts SET state='claimed',events_json=NULL,result_json=NULL WHERE message_id='accepted'").execute(&server.pool).await.unwrap();
    let before = model.calls.lock().unwrap().len();
    let recovered = send(&server, "归纳会议议题", "accepted", None)
        .await
        .unwrap();
    assert!(
        recovered.iter().any(
            |e| matches!(e,ServerNotification::TurnCompleted(c) if c.reply.contains(&task.id))
        )
    );
    send(&server, "归纳会议议题", "accepted", None)
        .await
        .unwrap();
    assert_eq!(model.calls.lock().unwrap().len(), before);
    assert_eq!(server.tasks.list(5).await.unwrap().len(), 1);
    assert_eq!(server.runs.count_by_task(&task.id).await.unwrap(), 1);
}
