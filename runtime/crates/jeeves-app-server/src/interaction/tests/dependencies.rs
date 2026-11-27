use super::*;
use jeeves_protocol::TaskStartParams;
use jeeves_provider::mock::Mock;
use jeeves_state::{TaskStatus, dispatch::TaskDispatch};

#[tokio::test]
async fn a_chat_assignment_waits_for_an_existing_tasks_verified_delivery() {
    let (home, server, _) = setup().await;
    let prerequisite = crate::task::create(
        &server,
        serde_json::from_value(json!({
            "sessionId": MAIN_SESSION, "title": "收集凭证", "cwd": home.path(),
            "acceptance": [{"kind": "fileExists", "path": "evidence.txt"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap()
    .task;
    let instruction = "等「收集凭证」验收通过后再整理摘要。";
    install(
        &server,
        vec![
            Mock::tool_use(
                "dependency",
                "task",
                json!({
                    "action": "create", "items": [{
                        "title": "整理摘要", "instruction": instruction, "after_task_ids": [prerequisite.id]
                    }]
                }),
            ),
            Mock::text("已排队，等待前置事项验收。"),
        ],
    );
    send(&server, instruction, "dependency-from-chat", None)
        .await
        .unwrap();
    let dependent = server
        .tasks
        .list_for_session(MAIN_SESSION, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.title == "整理摘要")
        .unwrap();
    let queue = TaskDispatch::new(server.pool.clone());
    assert_eq!(
        queue.get(&dependent.id).await.unwrap().unwrap().state,
        "pending"
    );
    assert_eq!(
        queue.unresolved_dependencies(&dependent.id).await.unwrap(),
        std::slice::from_ref(&prerequisite.id)
    );
    assert_eq!(server.runs.count_by_task(&dependent.id).await.unwrap(), 0);
    assert_eq!(
        server.runs.count_by_task(&prerequisite.id).await.unwrap(),
        0
    );
    assert!(queue.claim().await.unwrap().is_none());

    std::fs::write(home.path().join("evidence.txt"), "fixture delivery").unwrap();
    install(
        &server,
        vec![Mock::text("凭证已在目录中。"), Mock::text("摘要已整理。")],
    );
    let (sink, _) = EventSink::channel();
    crate::task::start(
        &server,
        &sink,
        TaskStartParams {
            task_id: prerequisite.id.clone(),
            message: "核对 evidence.txt 凭证文件".into(),
            attachments: None,
        },
    )
    .await
    .unwrap();
    wait(&server, &dependent.id).await;

    assert_eq!(
        server
            .tasks
            .get(&dependent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Done
    );
    let receipt = server
        .completions
        .latest_delivery(&prerequisite.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "done");
    assert_eq!(server.runs.count_by_task(&dependent.id).await.unwrap(), 1);
    assert!(
        queue
            .unresolved_dependencies(&dependent.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_model_cannot_add_a_prerequisite_missing_from_the_assignment() {
    let (_home, server, _) = setup().await;
    let prerequisite = crate::task::create(
        &server,
        serde_json::from_value(json!({
            "sessionId": MAIN_SESSION, "title": "收集凭证"
        }))
        .unwrap(),
    )
    .await
    .unwrap()
    .task;
    let instruction = "整理摘要。";
    install(
        &server,
        vec![
            Mock::tool_use(
                "invented-dependency",
                "task",
                json!({
                    "action": "create", "items": [{
                        "title": "整理摘要", "instruction": instruction, "after_task_ids": [prerequisite.id]
                    }]
                }),
            ),
            Mock::text("可重新确认任务。"),
        ],
    );
    send(&server, instruction, "invented-dependency", None)
        .await
        .unwrap();
    let tasks = server
        .tasks
        .list_for_session(MAIN_SESSION, 10)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, prerequisite.id);
}

fn install(server: &Server, responses: Vec<CreateResponse>) {
    server.install_provider(Resolved {
        client: Arc::new(Mock::streaming(responses)),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32_000,
    });
}
