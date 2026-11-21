use super::*;

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
