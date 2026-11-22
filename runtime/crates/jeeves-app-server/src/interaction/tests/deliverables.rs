use super::*;
use jeeves_provider::mock::Mock;
use std::path::Path;

const REQUEST: &str =
    "明天要和同事对一下这次事故。能帮我把复盘写进 review.md 吗？报告里得写「12 次事件」。";
const TITLE: &str = "交付事故复盘";

pub(super) fn reply(is_main: bool, message: &str) -> Option<CreateResponse> {
    if is_main && message == REQUEST {
        let handoff = |id, path| {
            Mock::tool_use(
                id,
                "task",
                json!({
                    "action":"create", "items":[{"title":TITLE,"instruction":REQUEST,
                        "file_checks":[{"path":path,"contains":"12 次事件"}]}]
                }),
            )
        };
        let mut calls = handoff("invented-path", "unspecified.md");
        calls
            .content
            .extend(handoff("review-task", "review.md").content);
        return Some(calls);
    }
    if !is_main && message.contains(REQUEST) {
        if message.contains("Acceptance did not pass") {
            return Some(Mock::tool_use(
                "deliver",
                "write_file",
                json!({
                    "path":"review.md", "content":"本期共有 12 次事件。"
                }),
            ));
        }
        return Some(Mock::text("报告已完成。"));
    }
    None
}

#[tokio::test]
async fn a_main_chat_deliverable_is_checked_and_repaired_in_the_same_task() {
    let (_home, server, _) = setup().await;
    let (sink, mut frames) = EventSink::channel();
    start(
        &server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id: "review".into(),
            message: REQUEST.into(),
            task_id: None,
            cwd: None,
            attachments: None,
            source: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap();
    let tasks = server.tasks.list(10).await.unwrap();
    assert_eq!(
        tasks.len(),
        1,
        "the valid handoff must create exactly one task"
    );
    assert_eq!(tasks[0].title, TITLE);
    let mut rejected_path = false;
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = frames.recv().await {
            match frame {
                Frame::Notification(ServerNotification::ToolCompleted(call))
                    if call.tool_use_id == "invented-path" =>
                {
                    rejected_path = call.output.starts_with("Error:");
                }
                Frame::Notification(ServerNotification::ApprovalRequested(ask)) => return ask,
                _ => {}
            }
        }
        panic!("worker closed before requesting approval for the correction");
    })
    .await
    .unwrap();
    assert!(rejected_path);
    assert!(server.answer_approval(
        &approval.turn_id,
        &approval.request_id,
        true,
        RememberScope::Once
    ));
    let id = &tasks[0].id;
    wait(&server, id).await;
    let task = server.tasks.get(id).await.unwrap().unwrap();
    assert_eq!(task.status, jeeves_state::TaskStatus::Done);
    assert!(task.goal_condition.is_none());
    assert_eq!(task.rounds_used, 2);
    let path = Path::new(task.cwd.as_deref().unwrap()).join("review.md");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "本期共有 12 次事件。"
    );
    let receipts = jeeves_state::verification::Verifications::new(server.pool.clone());
    let latest = receipts.latest(id).await.unwrap().unwrap();
    let receipt_id = latest.id.clone();
    let first = receipts
        .get(latest.previous_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    for (record, passed) in [(first, false), (latest, true)] {
        let checks: serde_json::Value = serde_json::from_str(&record.checks_json).unwrap();
        assert_eq!(checks[0]["passed"], passed);
        let criteria: serde_json::Value = serde_json::from_str(&record.acceptance_json).unwrap();
        assert_eq!(criteria[0]["kind"], "fileContains");
        assert_eq!(criteria[0]["path"], "review.md");
        assert_eq!(criteria[0]["pattern"], "12 次事件");
    }

    send(&server, "再用一句话总结。", "follow-up", Some(id.clone()))
        .await
        .unwrap();
    wait(&server, id).await;
    let follow_up = server.tasks.get(id).await.unwrap().unwrap();
    assert_eq!(follow_up.acceptance_json, "[]");
    assert_eq!(follow_up.checks_json, "[]");
    assert_eq!(server.runs.count_by_task(id).await.unwrap(), 2);
    assert!(receipts.get(&receipt_id).await.unwrap().is_some());
    let outcomes: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT status,verification_id FROM task_completions WHERE task_id=? ORDER BY id",
    )
    .bind(id)
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(
        outcomes,
        vec![
            ("done".into(), Some(receipt_id)),
            ("produced_unverified".into(), None)
        ]
    );
}

#[tokio::test]
async fn a_chat_follow_up_cannot_clear_an_explicit_task_api_contract() {
    let (home, server, _) = setup().await;
    let workspace = home.path().join("work");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("report.txt"), "delivered").unwrap();
    let task = crate::task::create(
        &server,
        serde_json::from_value(json!({
            "sessionId": MAIN_SESSION, "title":"Deliver report", "cwd":workspace,
            "acceptance":[{"kind":"fileExists","path":"report.txt"}]
        }))
        .unwrap(),
    )
    .await
    .unwrap()
    .task;
    let (sink, _) = EventSink::channel();
    crate::task::start(
        &server,
        &sink,
        jeeves_protocol::TaskStartParams {
            task_id: task.id.clone(),
            message: "Inspect the report.".into(),
            attachments: None,
        },
    )
    .await
    .unwrap();
    wait(&server, &task.id).await;
    let error = send(
        &server,
        "Change the assignment",
        "amend",
        Some(task.id.clone()),
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("completed conversational task"));
    let row = server.tasks.get(&task.id).await.unwrap().unwrap();
    assert_eq!(row.status, jeeves_state::TaskStatus::Done);
    assert_eq!(server.runs.count_by_task(&task.id).await.unwrap(), 1);
    assert!(row.acceptance_json.contains("report.txt"));
}
