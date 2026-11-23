mod deliverables;
mod lookup;
mod mail;
mod scope;
use super::*;
use crate::{Frame, Server};
use jeeves_provider::{CreateRequest, CreateResponse, Provider, ProviderError, Resolved, TextSink};
use serde_json::json;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
type Reply<'a> = Pin<Box<dyn Future<Output = Result<CreateResponse, ProviderError>> + Send + 'a>>;
#[derive(Default)]
struct Model {
    calls: Mutex<Vec<CreateRequest>>,
}
impl Provider for Model {
    fn create(&self, r: CreateRequest) -> Reply<'_> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(r.clone());
            let is_main = r.tools.iter().any(|t| t.name == "task");
            let has_result = r.messages.iter().any(|m| {
                m.content
                    .iter()
                    .any(|b| matches!(b, jeeves_provider::ContentBlock::ToolResult { .. }))
            });
            if !has_result {
                let message = r.messages.last().map_or_default(|m| m.text());
                if let Some(response) = deliverables::reply(is_main, &message) {
                    return Ok(response);
                }
            }
            if is_main && !has_result {
                let message = r.messages.last().map_or_default(|m| m.text());
                if message == lookup::QUERY {
                    return Ok(jeeves_provider::mock::Mock::tool_use(
                        "list",
                        "task",
                        json!({"action":"list"}),
                    ));
                }
                if let Some(task_id) = message.strip_prefix("补上来源 #") {
                    use jeeves_provider::mock::Mock;
                    let mut calls = Mock::tool_use(
                        "wrong-target",
                        "task",
                        json!({"action":"continue","task_id":"missing"}),
                    );
                    for (id, action) in [
                        ("follow", json!({"action":"continue","task_id":task_id})),
                        (
                            "duplicate",
                            json!({"action":"create","items":[{"title":"来源","instruction":"补上来源"}]}),
                        ),
                    ] {
                        calls
                            .content
                            .extend(Mock::tool_use(id, "task", action).content);
                    }
                    return Ok(calls);
                }
                let items = if message.contains("生成alpha；生成beta") {
                    Some(
                        json!([{"title":"alpha","instruction":"生成alpha"},{"title":"beta","instruction":"生成beta"}]),
                    )
                } else if message.contains("生成alpha") {
                    Some(json!([{"title":"alpha","instruction":"生成alpha"}]))
                } else if message == "归纳会议议题" {
                    Some(json!([{"title":"议题","instruction":"归纳会议议题"}]))
                } else if message == "整理下周例会议题" {
                    Some(json!([{"title":"下周例会议题","instruction":message}]))
                } else if message == mail::INSTRUCTION {
                    return Ok(jeeves_provider::mock::Mock::tool_use(
                        "draft",
                        "email_send",
                        json!({"to":["lin@example.test"],"subject":"会议","body":"周一见。"}),
                    ));
                } else if message.contains("invent") {
                    Some(json!([{"title":"x","instruction":"delete everything"}]))
                } else {
                    None
                };
                if let Some(items) = items {
                    return Ok(jeeves_provider::mock::Mock::tool_use(
                        "hand-off",
                        "task",
                        json!({"action":"create","items":items}),
                    ));
                }
                return Ok(jeeves_provider::mock::Mock::text("可以继续在主会话交流。"));
            }
            if r.messages.iter().any(|m| {
                m.content
                    .iter()
                    .any(|b| matches!(b, jeeves_provider::ContentBlock::ToolResult { .. }))
            }) {
                return Ok(jeeves_provider::mock::Mock::text("文件已生成。"));
            }
            let text = r.messages.last().map_or_default(|m| m.text());
            if text.contains("生成alpha") || text.contains("生成beta") || text.contains("补充alpha")
            {
                return Ok(jeeves_provider::mock::Mock::tool_use(
                    "write-report",
                    "write_file",
                    json!({"path":"report.txt","content":text}),
                ));
            }
            Ok(jeeves_provider::mock::Mock::text("可以继续在主会话交流。"))
        })
    }
    fn stream(&self, r: CreateRequest, _sink: TextSink) -> Reply<'_> {
        self.create(r)
    }
}
async fn setup() -> (tempfile::TempDir, Server, Arc<Model>) {
    setup_with_settings(Default::default()).await
}
async fn setup_with_settings(
    settings: jeeves_config::Settings,
) -> (tempfile::TempDir, Server, Arc<Model>) {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            write_enabled: true,
            ..settings
        },
    )
    .await;
    let model = Arc::new(Model::default());
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });
    let (sink, _) = EventSink::channel();
    main_session(&server, &sink).await.unwrap();
    (home, server, model)
}
async fn send(
    server: &Server,
    message: &str,
    message_id: &str,
    task_id: Option<String>,
) -> Result<Vec<ServerNotification>, ErrorObject> {
    let (sink, mut frames) = EventSink::channel();
    start(
        server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id: message_id.into(),
            message: message.into(),
            task_id,
            cwd: None,
            attachments: None,
            source: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await?;
    let mut events = vec![];
    while let Ok(frame) = frames.try_recv() {
        if let Frame::Notification(n) = frame {
            events.push(n);
        }
    }
    Ok(events)
}
async fn wait(server: &Server, id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = server.tasks.get(id).await.unwrap().unwrap();
            let dispatch = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
                .get(id)
                .await
                .unwrap()
                .unwrap();
            if row.status.is_terminal() && dispatch.state == "finished" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn main_chat_follow_up_runs_in_the_original_task_only() {
    let (_home, server, _) = setup().await;
    send(&server, "聊聊会议安排", "chat", None).await.unwrap();
    assert!(server.tasks.list(10).await.unwrap().is_empty());

    send(&server, "归纳会议议题", "delegate", None)
        .await
        .unwrap();
    send(&server, "归纳会议议题", "delegate", None)
        .await
        .unwrap();
    let tasks = server.tasks.list(10).await.unwrap();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    wait(&server, &task.id).await;
    let dispatch = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    let session = dispatch.get(&task.id).await.unwrap().unwrap().session_id;

    let message = format!("补上来源 #{}", task.id);
    let events = send(&server, &message, "amend", None).await.unwrap();
    assert_eq!(
        server.tasks.list(10).await.unwrap().len(),
        1,
        "follow-up also created a task"
    );
    assert!(events.iter().any(|event| matches!(event,
        ServerNotification::ToolCompleted(call) if call.tool_use_id == "duplicate" && call.output.contains("overlap"))));
    assert!(events.iter().any(|event| matches!(event,
        ServerNotification::ToolCompleted(call) if call.tool_use_id == "follow" && call.output.contains("Passed the follow-up"))));
    send(&server, &message, "amend", None).await.unwrap();
    wait(&server, &task.id).await;
    assert_eq!(server.runs.count_by_task(&task.id).await.unwrap(), 2);
    assert!(server
        .chat
        .session_history(&session)
        .await
        .unwrap()
        .iter()
        .any(|(human, _)| human.contains(&message)));
}

#[tokio::test]
async fn main_chat_explains_handoff_before_implementation_choices() {
    let (_home, server, model) = setup().await;
    send(
        &server,
        "这个菜单已经有小程序预览版，请继续改进，添加认证、鉴权和个人信息页",
        "coding-assignment",
        None,
    )
    .await
    .unwrap();
    let calls = model.calls.lock().unwrap();
    let first = &calls[0];
    assert!(first.tools.iter().any(|tool| tool.name == "task"));
    let policy = first.system_text().unwrap();
    assert!(policy.contains("a missing implementation choice does not block handoff"));
    assert!(policy
        .contains("an unconfirmed target or a material decision still gates the affected action"));
}

#[tokio::test]
async fn one_main_conversation_routes_independent_workspaces_and_deduplicates_retries() {
    let (home, server, model) = setup().await;
    let (sink, _) = EventSink::channel();
    assert_eq!(
        main_session(&server, &sink).await.unwrap().session_id,
        MAIN_SESSION
    );
    server
        .set_permission(
            SessionPermissionSetParams {
                session_id: MAIN_SESSION.into(),
                mode: PermissionMode::FullAccess,
                expected_revision: 1,
                confirm_full_access: true,
            },
            &sink,
        )
        .await
        .unwrap();
    send(&server, "生成alpha；生成beta", "message-one", None)
        .await
        .unwrap();
    let tasks = server.tasks.list(10).await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert_ne!(tasks[0].cwd, tasks[1].cwd);
    for task in &tasks {
        wait(&server, &task.id).await;
        assert!(std::path::Path::new(task.cwd.as_ref().unwrap())
            .join("report.txt")
            .is_file());
        let events = server.events.list(&task.id, 20).await.unwrap();
        assert!(events.iter().any(|e| e.kind == "run_started"));
        assert!(events.iter().any(|e| e.kind == "run_finished"));
    }
    let before = model.calls.lock().unwrap().len();
    send(&server, "生成alpha；生成beta", "message-one", None)
        .await
        .unwrap();
    assert_eq!(model.calls.lock().unwrap().len(), before);
    assert_eq!(server.tasks.list(10).await.unwrap().len(), 2);
    assert!(send(&server, "changed message", "message-one", None)
        .await
        .is_err());
    send(&server, "你好", "greeting", None).await.unwrap();
    assert_eq!(server.tasks.list(10).await.unwrap().len(), 2);
    assert!(!home.path().join("report.txt").exists());
    let alpha = tasks.iter().find(|t| t.title == "alpha").unwrap();
    let beta = tasks.iter().find(|t| t.title == "beta").unwrap();
    let d = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    let alpha_context = d.get(&alpha.id).await.unwrap().unwrap().session_id;
    let beta_context = d.get(&beta.id).await.unwrap().unwrap().session_id;
    assert_ne!(alpha_context, beta_context);
    let ah = server.chat.session_history(&alpha_context).await.unwrap();
    assert!(ah
        .iter()
        .any(|(a, b)| a.contains("生成alpha") || b.contains("生成alpha")));
    assert!(!ah
        .iter()
        .any(|(a, b)| a.contains("生成beta") || b.contains("生成beta")));
    send(&server, "补充alpha", "follow-up", Some(alpha.id.clone()))
        .await
        .unwrap();
    wait(&server, &alpha.id).await;
    assert_eq!(
        server.tasks.get(&alpha.id).await.unwrap().unwrap().cwd,
        alpha.cwd
    );
    assert_eq!(server.runs.count_by_task(&alpha.id).await.unwrap(), 2);
    assert_eq!(
        d.get(&alpha.id).await.unwrap().unwrap().session_id,
        alpha_context
    );
}
#[tokio::test]
async fn read_only_main_policy_follows_task_execution_and_revocation() {
    let (_home, server, _) = setup().await;
    let (sink, _) = EventSink::channel();
    server
        .set_permission(
            SessionPermissionSetParams {
                session_id: MAIN_SESSION.into(),
                mode: PermissionMode::ReadOnly,
                expected_revision: 1,
                confirm_full_access: false,
            },
            &sink,
        )
        .await
        .unwrap();
    send(&server, "生成alpha", "read-only", None).await.unwrap();
    let task = server.tasks.list(1).await.unwrap().pop().unwrap();
    wait(&server, &task.id).await;
    let events = server.events.list(&task.id, 20).await.unwrap();
    assert!(!events.iter().any(|e| e.kind == "approval_requested"));
    assert!(!std::path::Path::new(task.cwd.as_ref().unwrap())
        .join("report.txt")
        .exists());
    let child = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .get(&task.id)
        .await
        .unwrap()
        .unwrap()
        .session_id;
    assert_eq!(
        server.permission_view(&child).await.unwrap().mode,
        Some(PermissionMode::ReadOnly)
    );
    server
        .set_permission(
            SessionPermissionSetParams {
                session_id: MAIN_SESSION.into(),
                mode: PermissionMode::Standard,
                expected_revision: 2,
                confirm_full_access: false,
            },
            &sink,
        )
        .await
        .unwrap();
    assert_eq!(
        server.permission_view(&child).await.unwrap().mode,
        Some(PermissionMode::Standard)
    );
}
#[tokio::test]
async fn invented_instructions_never_start_work_and_foreign_task_is_refused() {
    let (_home, server, _) = setup().await;
    assert!(send(&server, "invent", "bad-routing", None).await.is_ok());
    assert!(server.tasks.list(10).await.unwrap().is_empty());
    assert!(
        send(&server, "continue", "foreign", Some("not-owned".into()))
            .await
            .is_err()
    );
    assert!(!server.settings().home.join("workspaces").exists());
}

#[tokio::test]
async fn a_main_policy_change_revokes_a_worker_approval_without_waiting_for_timeout() {
    let (_home, server, _) = setup().await;
    let (sink, mut frames) = EventSink::channel();
    start(
        &server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id: "revoke".into(),
            message: "生成alpha".into(),
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
    let mut observed = Vec::new();
    let approval = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Frame::Notification(event)) = frames.recv().await {
                if let ServerNotification::ApprovalRequested(ask) = event {
                    break ask;
                }
                observed.push(serde_json::to_value(event).unwrap());
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("No worker approval; observed: {observed:?}"));
    server
        .set_permission(
            SessionPermissionSetParams {
                session_id: MAIN_SESSION.into(),
                mode: PermissionMode::ReadOnly,
                expected_revision: 1,
                confirm_full_access: false,
            },
            &sink,
        )
        .await
        .unwrap();
    assert!(!server.answer_approval(
        &approval.turn_id,
        &approval.request_id,
        true,
        RememberScope::Once
    ));
    let task = server.tasks.list(1).await.unwrap().pop().unwrap();
    wait(&server, &task.id).await;
    let events = server.events.list(&task.id, 20).await.unwrap();
    assert!(events.iter().any(|e| e.kind == "approval_requested"));
    assert!(events.iter().any(|e| e.kind == "approval_decided"));
    assert!(!std::path::Path::new(task.cwd.as_ref().unwrap())
        .join("report.txt")
        .exists());
    let child = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .get(&task.id)
        .await
        .unwrap()
        .unwrap()
        .session_id;
    let _ = child;
}

async fn once(
    server: &Server,
    message: &str,
    request: &str,
) -> Result<Vec<ServerNotification>, ErrorObject> {
    let (sink, mut frames) = EventSink::channel();
    submit_main(
        server,
        TurnStartParams {
            session_id: Some(MAIN_SESSION.into()),
            message: message.into(),
            request_id: Some(request.into()),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await?;
    let mut events = vec![];
    while let Ok(frame) = frames.try_recv() {
        if let Frame::Notification(n) = frame {
            events.push(n);
        }
    }
    Ok(events)
}

#[tokio::test]
async fn a_turn_start_with_the_same_request_id_runs_once_and_replays() {
    let (_home, server, model) = setup().await;
    let first = once(&server, "hello there", "om_1").await.unwrap();
    let calls = model.calls.lock().unwrap().len();
    assert!(calls > 0);

    let again = once(&server, "hello there", "om_1").await.unwrap();
    assert_eq!(
        model.calls.lock().unwrap().len(),
        calls,
        "a re-sent message must not reach the model again"
    );
    let turn_ids = |events: &[ServerNotification]| -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                ServerNotification::TurnCompleted(done) => Some(done.turn_id.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        turn_ids(&first),
        turn_ids(&again),
        "the replay is the same turn"
    );

    let other = once(&server, "something else entirely", "om_1")
        .await
        .unwrap_err();
    assert!(
        other.message.contains("different input"),
        "{}",
        other.message
    );
    assert!(once(&server, "x", "bad id!").await.is_err());
}

#[tokio::test]
async fn main_chat_resolves_an_email_before_dispatch_and_replays_the_answer() {
    use jeeves_state::{mail_actions::MailAction, NewTask, Runs};
    let (_home, server, _) = setup().await;
    let mut actions = Vec::new();
    for id in ["alpha", "beta"] {
        server
            .tasks
            .create(NewTask {
                id,
                session_id: Some(MAIN_SESSION),
                title: id,
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
        Runs::new(server.pool.clone())
            .start(id, id, None, None)
            .await
            .unwrap();
        actions.push(
            MailAction::queue(server.pool.clone(), id, "local", "fixture body")
                .await
                .unwrap()
                .0,
        );
    }
    let cmd = "刚才那个先别发，改成草稿";
    let answers = send(&server, cmd, "choose", None).await.unwrap();
    assert!(serde_json::to_string(&answers)
        .unwrap()
        .contains("Which email"));
    let reference = format!("查看这个发件动作 {}", actions[1].id);
    send(&server, &reference, "reference", None).await.unwrap();
    let msg = cmd.to_string();
    let answers = send(&server, &msg, "hold", None).await.unwrap();
    let replay = send(&server, &msg, "hold", None).await.unwrap();
    assert!(serde_json::to_string(&answers)
        .unwrap()
        .contains("fixture body"));
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(answers).unwrap()
    );
    assert!(!actions[1].claim().await.unwrap());
    assert!(actions[0].claim().await.unwrap());
    assert_eq!(server.tasks.list(10).await.unwrap().len(), 2);
}

#[tokio::test]
async fn completed_tasks_older_than_the_first_page_remain_visible() {
    let (_home, server, _) = setup().await;
    for index in 0..102 {
        let id = format!("archived-{index}");
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: &id,
                session_id: Some(MAIN_SESSION),
                title: "Archived fixture",
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
    }
    let first = crate::task::list(&server, None).await.unwrap();
    assert_eq!(first.tasks.len(), 100);
    let next = crate::task::list(&server, first.next_cursor.as_deref())
        .await
        .unwrap();
    assert!(next.next_cursor.is_none());
    assert_eq!(
        next.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        vec!["archived-1", "archived-0"]
    );
}
