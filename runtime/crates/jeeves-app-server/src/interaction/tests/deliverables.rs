use super::*;
use jeeves_provider::mock::Mock;
use std::path::Path;

const REQUEST: &str =
    "明天要和同事对一下这次事故。能帮我把复盘写进 review.md 吗？报告里得写「12 次事件」。";
const TITLE: &str = "交付事故复盘";

/// A main-chat follow-up that names its own deliverable: `<FOLLOW_UP> #<task id>`.
const FOLLOW_UP: &str = "在 review.md 末尾补一句「已复核」";

/// A request whose outcome no file check can measure, so the model drafts a "done when" line.
const DRAFTED: &str = "帮我把这次事故的责任边界理清楚，我自己判断够不够清楚。";
/// The sentence the model drafts for that request; it stays inert until the person confirms it.
pub(super) const DRAFT: &str = "每一条责任都写明了归属的团队";

pub(super) fn reply(is_main: bool, message: &str) -> Option<CreateResponse> {
    if is_main && message == DRAFTED {
        return Some(Mock::tool_use(
            "drafted-goal",
            "task",
            json!({
                "action":"create",
                "items":[{"title":"责任边界","instruction":DRAFTED,"goal_draft":DRAFT}]
            }),
        ));
    }
    if !is_main && message.contains(DRAFTED) {
        return Some(Mock::text("已理清。"));
    }
    if is_main
        && let Some(task_id) = message
            .strip_prefix(FOLLOW_UP)
            .and_then(|rest| rest.strip_prefix(" #"))
    {
        return Some(Mock::tool_use(
            "follow-up-checks",
            "task",
            json!({
                "action":"continue", "task_id":task_id, "instruction":FOLLOW_UP,
                "acceptance":[{"kind":"fileContains","path":"review.md","pattern":"已复核"}]
            }),
        ));
    }
    if !is_main && message.contains(FOLLOW_UP) {
        if message.contains("Acceptance did not pass") {
            return Some(Mock::tool_use(
                "amend",
                "write_file",
                json!({"path":"review.md", "content":"本期共有 12 次事件。已复核。"}),
            ));
        }
        return Some(Mock::text("已补上。"));
    }
    if is_main && message == REQUEST {
        let handoff = |id, path| {
            Mock::tool_use(
                id,
                "task",
                json!({
                    "action":"create", "items":[{"title":TITLE,"instruction":REQUEST,
                        "acceptance":[{"kind":"fileContains","path":path,"pattern":"12 次事件"}]}]
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

#[tokio::test]
async fn a_main_chat_follow_up_with_its_own_checks_is_verified_in_the_same_task() {
    let (home, server, _) = setup().await;
    let workspace = home.path().join("work");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("review.md"), "本期共有 12 次事件。").unwrap();
    let task = crate::task::create(
        &server,
        serde_json::from_value(json!({"sessionId": MAIN_SESSION, "title":"复盘", "cwd":workspace}))
            .unwrap(),
    )
    .await
    .unwrap()
    .task;
    let (sink, mut frames) = EventSink::channel();
    crate::task::start(
        &server,
        &sink,
        jeeves_protocol::TaskStartParams {
            task_id: task.id.clone(),
            message: "检查复盘。".into(),
            attachments: None,
        },
    )
    .await
    .unwrap();
    wait(&server, &task.id).await;

    start(
        &server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id: "follow-up".into(),
            message: format!("{FOLLOW_UP} #{}", task.id),
            task_id: None,
            cwd: None,
            attachments: None,
            source: None,
        },
        RequestId::Number(2),
        &sink,
    )
    .await
    .unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = frames.recv().await {
            if let Frame::Notification(ServerNotification::ApprovalRequested(ask)) = frame {
                return ask;
            }
        }
        panic!("the correction never asked to write");
    })
    .await
    .unwrap();
    assert!(server.answer_approval(
        &approval.turn_id,
        &approval.request_id,
        true,
        RememberScope::Once
    ));
    wait(&server, &task.id).await;

    let row = server.tasks.get(&task.id).await.unwrap().unwrap();
    assert_eq!(row.status, jeeves_state::TaskStatus::Done);
    assert!(
        row.acceptance_json.contains("已复核"),
        "{}",
        row.acceptance_json
    );
    assert_eq!(
        row.rounds_used, 2,
        "the first follow-up round fails its check and is corrected"
    );
    let latest: (String, Option<String>) = sqlx::query_as(
        "SELECT status,verification_id FROM task_completions WHERE task_id=? ORDER BY id DESC LIMIT 1",
    )
    .bind(&task.id)
    .fetch_one(&server.pool)
    .await
    .unwrap();
    assert_eq!(latest.0, "done");
    assert!(
        latest.1.is_some(),
        "the follow-up delivery carries a verification receipt"
    );
}

#[tokio::test]
async fn a_drafted_goal_judges_nothing_until_the_person_confirms_it() {
    let (_home, server, _) = setup().await;
    send(&server, DRAFTED, "boundaries", None).await.unwrap();
    let task = server
        .tasks
        .list(20)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.title == "责任边界")
        .expect("the draft opened a task");
    assert_eq!(task.goal_draft.as_deref(), Some(DRAFT));
    assert_eq!(
        task.goal_condition, None,
        "a model's draft never activates on its own"
    );
    wait(&server, &task.id).await;
    let row = server.tasks.get(&task.id).await.unwrap().unwrap();
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "nothing was judged against the draft: {:?}",
        row.last_stop_reason
    );
    assert_eq!(row.goal_condition, None);

    let (sink, _) = EventSink::channel();
    let confirmed = crate::task::confirm_goal(
        &server,
        &sink,
        jeeves_protocol::TaskGoalConfirmParams {
            task_id: task.id.clone(),
            accept: true,
        },
    )
    .await
    .unwrap()
    .task;
    assert_eq!(confirmed.goal_condition.as_deref(), Some(DRAFT));
    assert_eq!(confirmed.goal_draft, None, "the card never asks twice");
    let again = crate::task::confirm_goal(
        &server,
        &sink,
        jeeves_protocol::TaskGoalConfirmParams {
            task_id: task.id.clone(),
            accept: true,
        },
    )
    .await
    .unwrap_err();
    assert!(
        again.message.contains("no drafted goal"),
        "{}",
        again.message
    );
}

#[tokio::test]
async fn a_declined_draft_leaves_the_task_unjudged() {
    let (_home, server, _) = setup().await;
    send(&server, DRAFTED, "boundaries", None).await.unwrap();
    let task = server
        .tasks
        .list(20)
        .await
        .unwrap()
        .into_iter()
        .find(|row| row.title == "责任边界")
        .expect("the draft opened a task");
    let (sink, _) = EventSink::channel();
    let answered = crate::task::confirm_goal(
        &server,
        &sink,
        jeeves_protocol::TaskGoalConfirmParams {
            task_id: task.id.clone(),
            accept: false,
        },
    )
    .await
    .unwrap()
    .task;
    assert_eq!(answered.goal_draft, None);
    assert_eq!(answered.goal_condition, None);
}
