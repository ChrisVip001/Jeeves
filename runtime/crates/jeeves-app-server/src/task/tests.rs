//! Long-task entity tests — one question per test, all through the real dispatch path:
//! does the task row tell the truth about what the human asked for?
//!
//! The interesting half is the pause/resume pair: pausing must **keep** the checkpoint
//! (a pause is not a stop), and continuing must resume it instead of replaying tools.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use jeeves_config::Settings;
use jeeves_protocol::{
    JsonRpcMessage, JsonRpcRequest, RequestId, ServerNotification, TaskEnvelope, TurnStartParams,
    codes,
};
use jeeves_provider::mock::Mock;
use jeeves_provider::{Message, Resolved, Role};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{EventSink, Frame, Server};

async fn boot(mock: Option<Arc<Mock>>) -> (Server, PathBuf) {
    boot_with(mock, false).await
}

async fn boot_with(mock: Option<Arc<Mock>>, plan_enabled: bool) -> (Server, PathBuf) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    let server = Server::boot(
        pool,
        Settings {
            home: home.clone(),
            write_enabled: true,
            plan_enabled,
            ..Settings::default()
        },
    )
    .await;
    if let Some(mock) = mock {
        server.set_resolved(Some(Resolved {
            context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
            vision: false,
            provider_id: "mock".to_string(),
            client: mock,
            model: "test-model".to_string(),
            small_model: "test-small".to_string(),
        }));
    }
    (server, home)
}

async fn call(
    server: &Server,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    if method == jeeves_protocol::methods::TASK_CREATE {
        let params = serde_json::from_value(params).map_err(|error| error.to_string())?;
        let task = super::create(server, params)
            .await
            .map_err(|error| error.message)?;
        return Ok(serde_json::to_value(task).unwrap());
    }
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: RequestId::Number(1),
        method: method.to_string(),
        params: Some(params),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::handle(server, request, EventSink::new(tx)).await;
    let mut response = None;
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Response(message) = frame {
            response = Some(message);
        }
    }
    match response.expect("every request has a reply") {
        JsonRpcMessage::Response(response) => Ok(response.result),
        JsonRpcMessage::Error(error) => Err(error.error.message),
    }
}

async fn create_task(
    server: &Server,
    session: &str,
    title: &str,
    goal: Option<&str>,
) -> TaskEnvelope {
    let result = call(
        server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({ "sessionId": session, "title": title, "goalCondition": goal }),
    )
    .await
    .expect("creating the task should succeed");
    serde_json::from_value(result).expect("TaskEnvelope")
}

#[tokio::test]
async fn several_tasks_may_be_open_in_one_session_and_the_newest_is_the_card() {
    let (server, _home) = boot(None).await;
    let first = create_task(
        &server,
        "default",
        "watch the server for three days",
        Some("watch it all three days"),
    )
    .await;

    let second = create_task(&server, "default", "another thing", None).await;
    assert_ne!(first.task.id, second.task.id);

    let open = server
        .tasks
        .list_active()
        .await
        .expect("list the active tasks");
    assert_eq!(open.len(), 2, "both are still there");

    let view = call(
        &server,
        jeeves_protocol::methods::TASK_VIEW,
        json!({ "sessionId": "default" }),
    )
    .await
    .expect("the board should answer");
    assert_eq!(
        view["task"]["title"],
        json!("another thing"),
        "the card shows the newest one"
    );

    create_task(&server, "other", "the third thing", None).await;
}

#[tokio::test]
async fn a_task_can_declare_its_tier_and_a_typo_is_refused() {
    let (server, _home) = boot(None).await;

    let error = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({ "sessionId": "default", "title": "read it through", "tier": "reasoining" }),
    )
    .await
    .expect_err("a misspelt tier must be refused on the spot");
    assert!(
        error.contains("reasoining"),
        "the error has to carry the string it received: {error}"
    );

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({ "sessionId": "default", "title": "read it through", "tier": "reasoning" }),
    )
    .await
    .expect("a legal tier should create the task");
    assert_eq!(created["task"]["tier"], json!("reasoning"));

    let id = created["task"]["id"].as_str().expect("it has an id");
    let row = server
        .tasks
        .get(id)
        .await
        .expect("query")
        .expect("the task is there");
    assert_eq!(
        row.tier.as_deref(),
        Some("reasoning"),
        "the tier lands on the task row, not on one round"
    );

    let (plain_server, _plain_home) = boot(None).await;
    let plain = create_task(&plain_server, "default", "a casual question", None).await;
    assert!(plain.task.tier.is_none());
}

#[tokio::test]
async fn setting_a_goal_creates_the_task_and_clearing_keeps_the_row() {
    let (server, _home) = boot(None).await;
    let row = server
        .set_task_goal("default", Some("get the tests green"))
        .await
        .expect("setting a goal should create the task");

    assert_eq!(row.goal_condition.as_deref(), Some("get the tests green"));
    assert_eq!(row.title, "get the tests green");

    server
        .set_task_goal("default", None)
        .await
        .expect("clearing the goal should succeed");
    let task = server
        .tasks
        .get(&row.id)
        .await
        .expect("query")
        .expect("still there");
    assert!(task.goal_condition.is_none(), "the goal is cleared");
    assert_eq!(
        task.status,
        jeeves_state::TaskStatus::Pending,
        "the task itself stays"
    );
}

#[tokio::test]
async fn a_single_turn_task_closes_done_when_its_turn_ends() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("done"),
        Mock::text("done"),
        Mock::text("done"),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let task = create_task(&server, "default", "check the logs", None).await;
    assert_eq!(task.task.status, "pending");

    let (tx, _rx) = mpsc::unbounded_channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("default".to_string()),
            message: "let us start".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect("the turn should finish");

    let row = server
        .tasks
        .get(&task.task.id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "a single-round task: done the moment the turn ends"
    );
    assert!(row.finished_at.is_some());
    assert!(row.current_turn_id.is_none(), "the heartbeat is cleared");
}

#[tokio::test]
async fn suspending_keeps_the_checkpoint_and_resuming_continues_it() {
    let mock = Arc::new(Mock::new(vec![Mock::text("carried on and finished")]));
    let (server, _home) = boot(Some(mock)).await;
    let task = create_task(
        &server,
        "default",
        "watch the server for three days",
        Some("watch it for all three days"),
    )
    .await;

    server
        .runs
        .start("t-pause", &task.task.id, Some("default"), Some("t-pause"))
        .await
        .expect("run");
    let messages = vec![
        Message::user_text("watch the server for me for three days"),
        Message {
            role: Role::Assistant,
            content: vec![jeeves_provider::ContentBlock::Text {
                text: "note it down first".to_string(),
            }],
        },
    ];
    server
        .checkpoints
        .save(
            "t-pause",
            "default",
            "watch the server for me for three days",
            &serde_json::to_string(&messages).expect("serialize"),
            1,
            "{}",
            Some("t-pause"),
        )
        .await
        .expect("write the checkpoint");
    server
        .tasks
        .set_running(&task.task.id, "t-pause")
        .await
        .expect("mark it running");
    server
        .checkpoints
        .finish("t-pause", "suspended")
        .await
        .expect("simulate the suspend half of finalize");

    let suspended = call(
        &server,
        jeeves_protocol::methods::TASK_SUSPEND,
        json!({ "taskId": task.task.id }),
    )
    .await
    .expect("suspending should succeed");
    assert_eq!(suspended["task"]["status"], json!("suspended"));
    assert!(
        suspended["task"]["suspendReason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()),
        "the suspension reason has to be recorded"
    );
    let checkpoint = server
        .checkpoints
        .get("t-pause")
        .await
        .expect("query")
        .expect("the checkpoint is still there");
    assert_eq!(
        checkpoint.status, "suspended",
        "a suspension keeps the checkpoint, not marked abandoned"
    );
    assert_eq!(
        server
            .tasks
            .get(&task.task.id)
            .await
            .expect("query")
            .expect("present")
            .current_turn_id
            .as_deref(),
        Some("t-pause"),
        "a suspended task remembers which round it stopped at"
    );

    let resumed = call(
        &server,
        jeeves_protocol::methods::TASK_RESUME,
        json!({ "taskId": task.task.id }),
    )
    .await
    .expect("resuming should succeed");
    assert_eq!(resumed["task"]["status"], json!("running"));

    let mut replied = false;
    for _ in 0..200 {
        replied = server
            .chat
            .session_history("default")
            .await
            .expect("read the conversation log")
            .iter()
            .any(|(_, content)| content.contains("carried on and finished"));
        if replied {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(replied, "the resumed reply reached the ledger");

    let checkpoint = server
        .checkpoints
        .get("t-pause")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(checkpoint.status, "done", "the resume ran to the end");
}

#[tokio::test]
async fn cancelling_closes_the_task_and_never_auto_continues() {
    let (server, _home) = boot(None).await;
    let task = create_task(
        &server,
        "default",
        "the thing that was stopped",
        Some("cannot finish it"),
    )
    .await;
    server
        .checkpoints
        .save(
            "t-stop",
            "default",
            "the thing that was stopped",
            "[]",
            1,
            "{}",
            None,
        )
        .await
        .expect("write the checkpoint");
    server
        .tasks
        .set_running(&task.task.id, "t-stop")
        .await
        .expect("mark it running");

    let cancelled = call(
        &server,
        jeeves_protocol::methods::TASK_CANCEL,
        json!({ "taskId": task.task.id }),
    )
    .await
    .expect("cancelling should succeed");
    assert_eq!(cancelled["task"]["status"], json!("cancelled"));

    let checkpoint = server
        .checkpoints
        .get("t-stop")
        .await
        .expect("query")
        .expect("present");
    assert_eq!(
        checkpoint.status, "abandoned",
        "a cancelled turn is never resumed automatically"
    );
    assert!(
        server
            .checkpoints
            .open_rows()
            .await
            .expect("read")
            .is_empty(),
        "no leftovers to resume"
    );
    assert!(
        server
            .tasks
            .active("default")
            .await
            .expect("query")
            .is_none(),
        "the session is free, so a new task can start"
    );
}

#[tokio::test]
async fn a_restart_marks_a_running_task_suspended_honestly() {
    let (server, home) = boot(None).await;
    let task = create_task(
        &server,
        "default",
        "run a long task",
        Some("until it is done"),
    )
    .await;
    server
        .tasks
        .set_running(&task.task.id, "t-live")
        .await
        .expect("mark it running");
    drop(server);

    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("reopen the store");
    let rebooted = Server::boot(
        pool,
        Settings {
            home: home.clone(),
            ..Settings::default()
        },
    )
    .await;
    let row = rebooted
        .tasks
        .get(&task.task.id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(row.status, jeeves_state::TaskStatus::Suspended);
    assert!(
        row.suspend_reason
            .as_deref()
            .is_some_and(|r| r.contains("the process restarted")),
        "the reason has to be stated plainly: {:?}",
        row.suspend_reason
    );
}

#[tokio::test]
async fn an_unknown_task_id_is_an_error_not_a_panic() {
    let (server, _home) = boot(None).await;
    let error = call(
        &server,
        jeeves_protocol::methods::TASK_RESUME,
        json!({ "taskId": "task-nope" }),
    )
    .await
    .expect_err("a task that does not exist must error");
    assert!(error.contains("no such task"), "{error}");

    let task = create_task(&server, "default", "completed", None).await;
    server
        .tasks
        .finish(&task.task.id, jeeves_state::TaskStatus::Done)
        .await
        .expect("finish it");
    let error = call(
        &server,
        jeeves_protocol::methods::TASK_RESUME,
        json!({ "taskId": task.task.id }),
    )
    .await
    .expect_err("a finished task cannot be resumed");
    assert_eq!(codes::INVALID_PARAMS, -32602);
    assert!(error.contains("suspended"), "{error}");
}

#[tokio::test]
async fn task_transitions_are_announced() {
    let (server, _home) = boot(None).await;
    let task = create_task(&server, "default", "a task that has to be broadcast", None).await;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);
    let params = jeeves_protocol::TaskActionParams {
        task_id: task.task.id.clone(),
        reason: Some("paused for the test".to_string()),
    };
    crate::task::suspend(&server, &sink, params)
        .await
        .expect("suspend it");

    let mut announced = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Notification(ServerNotification::TaskUpdated(updated)) = frame {
            announced.push(updated);
        }
    }
    assert!(
        !announced.is_empty(),
        "a status change has to be broadcast, not kept quiet"
    );
    assert_eq!(announced.last().expect("present").status, "suspended");
    assert_eq!(announced.last().expect("present").task_id, task.task.id);
}

async fn drive(server: &Server, session: &str, message: &str, cwd: Option<&str>) {
    let (tx, _rx) = mpsc::unbounded_channel();
    crate::turn::run_turn(
        server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some(session.to_string()),
            message: message.to_string(),
            stream: Some(true),
            cwd: cwd.map(str::to_string),
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect("the turn should finish");
}

#[tokio::test]
async fn a_task_can_exist_without_any_session() {
    let (server, _home) = boot(None).await;
    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({ "title": "tidy up the downloads directory", "acceptance": [
            { "kind": "fileExists", "path": "index.md" }
        ] }),
    )
    .await
    .expect("a task with no session should still be created");

    assert!(
        created["task"]["sessionId"].is_null(),
        "no session means no session"
    );
    let id = created["task"]["id"].as_str().expect("it has an id");
    assert_eq!(created["task"]["status"], json!("pending"));
    assert_eq!(
        created["task"]["acceptance"][0]["label"],
        json!("index.md exists")
    );
    assert_eq!(created["task"]["acceptance"][0]["min"], json!(1));

    let got = call(
        &server,
        jeeves_protocol::methods::TASK_GET,
        json!({ "taskId": id }),
    )
    .await
    .expect("looking it up by id should find it");
    assert_eq!(
        got["task"]["title"],
        json!("tidy up the downloads directory")
    );

    let listed = call(&server, jeeves_protocol::methods::TASK_LIST, json!({}))
        .await
        .expect("the list should succeed");
    assert_eq!(listed["tasks"].as_array().expect("an array").len(), 1);
}

#[tokio::test]
async fn a_task_whose_acceptance_fails_needs_a_human_not_a_done() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("skip"),
        Mock::text("the report is written, please take a look."),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "write a report",
            "maxRounds": 1,
            "cwd": workspace,
            "acceptance": [{ "kind": "fileExists", "path": "report.md" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::NeedsAttention,
        "acceptance not met means it cannot count as done: {:?}",
        row.last_stop_reason
    );
    let reason = row
        .last_stop_reason
        .as_deref()
        .expect("it has to say why it stopped");
    assert!(reason.contains("report.md does not exist"), "{reason}");

    let checks: serde_json::Value =
        serde_json::from_str(&row.checks_json).expect("the acceptance results");
    assert_eq!(checks[0]["passed"], json!(false));
    assert_eq!(checks[0]["label"], json!("report.md exists"));
}

#[tokio::test]
async fn a_condition_passes_only_after_the_file_really_exists() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("skip"),
        Mock::tool_use(
            "call_1",
            "write_file",
            json!({ "path": "report.md", "content": "verdict: ready to ship\n" }),
        ),
        Mock::text("written."),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "write a report",
            "cwd": workspace,
            "acceptance": [
                { "kind": "fileExists", "path": "report.md" },
                { "kind": "fileContains", "path": "report.md", "pattern": "verdict", "min": 1 }
            ],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "the file really is there, so it should complete: {:?}",
        row.last_stop_reason
    );
    assert!(row.finished_at.is_some());

    let artifacts = server
        .artifacts
        .list(&id, 10)
        .await
        .expect("list the deliverables");
    assert_eq!(artifacts.len(), 1);
    assert!(artifacts[0].path.ends_with("report.md"));
    assert!(artifacts[0].bytes > 0);

    let runs = server
        .runs
        .list_by_task(&id, 10)
        .await
        .expect("list the runs");
    assert_eq!(runs.len(), 1, "one turn is one run");
    assert_eq!(runs[0].status, "completed");
    assert!(runs[0].tools_ok >= 1, "successful calls are recorded");

    let steps = server
        .steps
        .list_by_run(&runs[0].id)
        .await
        .expect("list the steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].tool.as_deref(), Some("write_file"));
    assert_eq!(steps[0].status, "ok");
    let key = steps[0]
        .idempotency_key
        .as_deref()
        .expect("it has an idempotency key");
    assert!(
        key.ends_with(":1"),
        "the key carries the step number: {key}"
    );
    assert!(
        key.starts_with(&runs[0].id),
        "the key carries the turn: {key}"
    );
}

#[tokio::test]
async fn a_task_over_budget_stops_before_it_spends_more() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("skip"),
        Mock::text("do a little first."),
        Mock::text("do a little more."),
    ]));
    let (server, _home) = boot(Some(mock)).await;

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "take it slowly",
            "budget": { "tokens": 10 },
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(row.status, jeeves_state::TaskStatus::NeedsAttention);
    assert!(
        row.spent_tokens >= 10,
        "the spend has to be recorded: {}",
        row.spent_tokens
    );
    let reason = row
        .last_stop_reason
        .as_deref()
        .expect("it has to say which limit was hit");
    assert!(
        reason.contains("Over budget") && reason.contains("tokens"),
        "{reason}"
    );
}

#[tokio::test]
async fn each_turn_that_carries_the_task_is_its_own_run() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("skip"),
        Mock::text("first pass."),
        Mock::text("skip"),
        Mock::text("second pass."),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "do it in several goes",
            "maxRounds": 1,
            "cwd": workspace,
            "acceptance": [{ "kind": "fileExists", "path": "report.md" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "step one", None).await;
    assert_eq!(
        server
            .runs
            .count_by_task(&id)
            .await
            .expect("count the runs"),
        1,
        "the first pass is one run"
    );
    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(row.status, jeeves_state::TaskStatus::NeedsAttention);
    assert!(
        row.current_turn_id.is_none(),
        "the heartbeat has to be cleared"
    );

    drive(&server, "default", "step two", None).await;
    assert_eq!(
        server
            .runs
            .count_by_task(&id)
            .await
            .expect("count the runs"),
        2,
        "the second pass is a new run"
    );

    let runs = server
        .runs
        .list_by_task(&id, 10)
        .await
        .expect("list the runs");
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0].status, "waiting",
        "stopped at the acceptance check, waiting for a human"
    );
    assert!(runs[0].stop_reason.is_some());
}

#[tokio::test]
async fn each_round_keeps_its_own_step_so_tools_ok_counts_them_all() {
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use(
            "call_1",
            "write_file",
            json!({ "path": "a.md", "content": "A\n" }),
        ),
        Mock::text("the first one."),
        Mock::tool_use(
            "call_2",
            "write_file",
            json!({ "path": "b.md", "content": "B\n" }),
        ),
        Mock::text("both batches are done."),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "two rounds, two files written",
            "cwd": workspace,
            "acceptance": [{ "kind": "toolsOk", "tool": "write_file", "min": 2 }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "two real successful writes should complete it: {:?}",
        row.last_stop_reason
    );
    assert_eq!(
        row.rounds_used, 2,
        "the result comes from two rounds, not one"
    );

    let runs = server
        .runs
        .list_by_task(&id, 10)
        .await
        .expect("list the runs");
    let steps = server
        .steps
        .list_by_run(&runs[0].id)
        .await
        .expect("list the steps");
    assert_eq!(
        steps.len(),
        2,
        "one per round; the idempotency key swallowed neither"
    );
    let mut keys: Vec<&str> = steps
        .iter()
        .filter_map(|step| step.idempotency_key.as_deref())
        .collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(
        keys.len(),
        2,
        "the idempotency keys have to differ: {keys:?}"
    );
}

#[tokio::test]
async fn an_interrupted_round_suspends_the_task_instead_of_spending_another() {
    let mock = Arc::new(Mock::streaming(vec![
        Mock::text("skip"),
        Mock::text(
            "let me get the shape of it straight first, then I will start writing this report, and there is a lot more to say.",
        ),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "write a report",
            "maxRounds": 4,
            "cwd": workspace,
            "acceptance": [{ "kind": "fileExists", "path": "report.md" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);
    let runner = server.clone();
    let turn = tokio::spawn(async move {
        crate::turn::run_turn(
            &runner,
            TurnStartParams {
                request_id: None,
                attachments: None,
                session_id: Some("default".to_string()),
                message: "let us start".to_string(),
                stream: Some(true),
                cwd: None,
                tier: None,
            },
            RequestId::Number(1),
            &sink,
        )
        .await
    });

    let mut deltas = 0usize;
    let mut turn_id = String::new();
    while let Some(frame) = rx.recv().await {
        let Frame::Notification(notification) = frame else {
            continue;
        };
        match notification {
            ServerNotification::TurnStarted(started) => turn_id = started.turn_id,
            ServerNotification::TextDelta(_) => {
                deltas += 1;
                if deltas == 2 {
                    assert!(
                        server.interrupt_turn(&turn_id),
                        "the interrupt should hit the running turn"
                    );
                }
            }
            ServerNotification::TurnCompleted(_) => break,
            _ => {}
        }
    }
    turn.await
        .expect("the turn task should not crash")
        .expect("an interrupt is an ordinary ending, not an error");

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Suspended,
        "an interrupted task stops and waits for a human to resume: {:?}",
        row.last_stop_reason
    );
    assert_eq!(row.rounds_used, 1, "only the interrupted round was spent");
    let runs = server
        .runs
        .list_by_task(&id, 10)
        .await
        .expect("list the runs");
    assert_eq!(runs[0].status, "suspended");
}

#[tokio::test]
async fn the_planner_writes_plan_steps_without_touching_acceptance() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("{\"steps\": [\"write the skeleton\", \"then add the verdict\"]}"),
        Mock::tool_use(
            "call_1",
            "write_file",
            json!({ "path": "report.md", "content": "verdict: ready to ship\n" }),
        ),
        Mock::text("written."),
    ]));
    let (server, _home) = boot_with(Some(mock), true).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "write a release report",
            "cwd": workspace,
            "acceptance": [{ "kind": "fileExists", "path": "report.md" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(row.status, jeeves_state::TaskStatus::Done);
    let acceptance: serde_json::Value =
        serde_json::from_str(&row.acceptance_json).expect("the acceptance conditions");
    assert_eq!(acceptance.as_array().expect("an array").len(), 1);

    let planned = server.steps.list_plan(&id).await.expect("list the plan");
    assert_eq!(planned.len(), 2, "a two-step plan gets two rows");
    assert!(planned.iter().all(|step| step.kind == "plan"));
    assert!(planned.iter().all(|step| step.status == "planned"));
    assert!(
        planned.iter().all(|step| step.run_id.is_none()),
        "the plan does not belong to one run"
    );
    assert!(
        planned[0]
            .input_json
            .as_deref()
            .is_some_and(|json| json.contains("write the skeleton"))
    );

    let runs = server
        .runs
        .list_by_task(&id, 10)
        .await
        .expect("list the runs");
    let tool_steps = server
        .steps
        .list_by_run(&runs[0].id)
        .await
        .expect("list the steps");
    assert_eq!(tool_steps.len(), 1);
    assert_eq!(tool_steps[0].kind, "tool");
}

#[tokio::test]
async fn a_broken_plan_is_ignored_not_fatal() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("I think we could do it this way (and I am not giving you JSON)"),
        Mock::text("skip"),
        Mock::tool_use(
            "call_1",
            "write_file",
            json!({ "path": "report.md", "content": "verdict\n" }),
        ),
        Mock::text("written."),
    ]));
    let (server, _home) = boot_with(Some(mock), true).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().display().to_string();

    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default",
            "title": "write a report",
            "cwd": workspace,
            "acceptance": [{ "kind": "fileExists", "path": "report.md" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"]
        .as_str()
        .expect("it has an id")
        .to_string();

    drive(&server, "default", "let us start", None).await;

    let row = server
        .tasks
        .get(&id)
        .await
        .expect("query")
        .expect("still there");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "a broken planner does not affect the task itself"
    );
    assert!(
        server
            .steps
            .list_plan(&id)
            .await
            .expect("list the plan")
            .is_empty()
    );
}

#[tokio::test]
async fn observation_write_failures_stop_before_continuation_and_keep_known_spend() {
    for (table, action, requires_step) in [
        ("task_steps", "INSERT", false),
        ("task_steps", "UPDATE", false),
        ("task_artifacts", "INSERT", false),
        ("task_steps", "INSERT", true),
    ] {
        let mock = Arc::new(Mock::new(vec![
            Mock::tool_use(
                "write-once",
                "write_file",
                json!({"path":"report.md","content":"Observed output."}),
            ),
            Mock::text("The report is ready."),
        ]));
        let (server, home) = boot(Some(mock.clone())).await;
        let work = home.join("work");
        std::fs::create_dir(&work).unwrap();
        let acceptance = if requires_step {
            json!([{"kind":"toolsOk","tool":"write_file","min":1}])
        } else {
            json!([])
        };
        let task = call(
            &server,
            "task/create",
            json!({
                "sessionId":"recording", "title":"Persist observed work", "cwd":work,
                "maxRounds":3, "acceptance":acceptance,
            }),
        )
        .await
        .unwrap();
        sqlx::query(&format!(
            "CREATE TRIGGER fail_observation BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT, 'injected observation failure'); END"
        )).execute(&server.pool).await.unwrap();
        drive(
            &server,
            "recording",
            "Write the report once.",
            work.to_str(),
        )
        .await;
        let view = call(&server, "task/get", json!({"taskId":task["task"]["id"]}))
            .await
            .unwrap();
        assert_eq!(
            view["task"]["status"], "needs_attention",
            "{table} {action}"
        );
        assert!(
            view["task"]["lastStopReason"]
                .as_str()
                .unwrap()
                .contains("observation")
        );
        assert_eq!(view["task"]["spent"]["tokens"], 43);
        assert_eq!(view["task"]["spent"]["tools"], 1);
        assert_eq!(
            mock.received.lock().unwrap().len(),
            2,
            "Persistence failure must not trigger repair model calls"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("report.md")).unwrap(),
            "Observed output."
        );
        let completed: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task_completions WHERE status = 'done'")
                .fetch_one(&server.pool)
                .await
                .unwrap();
        assert_eq!(
            completed, 0,
            "Unrecorded work must not advertise completion"
        );
        let (turn_id, round_id): (String, String) = sqlx::query_as(
            "SELECT turn_id, round_id FROM turn_checkpoints WHERE session_id='recording'",
        )
        .fetch_one(&server.pool)
        .await
        .unwrap();
        let observation = crate::turn::observe_round(&server, &turn_id, &round_id, Some(&work))
            .await
            .unwrap();
        assert_eq!(observation.steps.len(), 1);
        sqlx::query("DROP TRIGGER fail_observation")
            .execute(&server.pool)
            .await
            .unwrap();
        let run = server.runs.for_turn(&turn_id).await.unwrap().unwrap();
        let recorder = jeeves_agent::observe::Recorder {
            steps: &server.steps,
            artifacts: &server.artifacts,
            events: &server.events,
            task_id: &run.task_id,
            run_id: Some(&run.id),
        };
        for _ in 0..2 {
            recorder.record(&observation).await.unwrap();
        }
        assert_eq!(server.steps.list_by_run(&run.id).await.unwrap().len(), 1);
        assert_eq!(
            server.artifacts.list(&run.task_id, 10).await.unwrap().len(),
            1
        );
        assert_eq!(mock.received.lock().unwrap().len(), 2);
        let task = server.tasks.get(&run.task_id).await.unwrap().unwrap();
        assert_eq!(task.spent_tokens, 43);
        assert_eq!(task.status, jeeves_state::TaskStatus::NeedsAttention);
    }
}

#[tokio::test]
async fn failed_model_round_charges_checkpoint_once_and_retains_accounting_faults() {
    for fail_accounting in [false, true] {
        let mock = Arc::new(Mock::with_outcomes(vec![
            Ok(Mock::tool_use(
                "write",
                "write_file",
                json!({"path":"report.md","content":"Completed before provider failure."}),
            )),
            Err(jeeves_provider::ProviderError::Api(
                "Injected provider failure".into(),
            )),
        ]));
        let (server, home) = boot(Some(mock.clone())).await;
        let work = home.join("work");
        std::fs::create_dir(&work).unwrap();
        let created = create_task(&server, "failed-accounting", "Keep known spend", None).await;
        if fail_accounting {
            sqlx::query("CREATE TRIGGER fail_round_charge BEFORE INSERT ON run_round_accounts BEGIN SELECT RAISE(FAIL,'injected accounting failure'); END").execute(&server.pool).await.unwrap();
        }
        let (sink, _rx) = EventSink::channel();
        let result = crate::turn::run_turn(
            &server,
            TurnStartParams {
                session_id: Some("failed-accounting".into()),
                message: "Write report.md, then verify it.".into(),
                cwd: Some(work.display().to_string()),
                ..Default::default()
            },
            RequestId::Number(1),
            &sink,
        )
        .await;
        let error = result.unwrap_err().message;
        assert!(
            error.contains(if fail_accounting {
                "Cannot charge reported model usage"
            } else {
                "Injected provider failure"
            }),
            "{error}"
        );
        let task = server.tasks.get(&created.task.id).await.unwrap().unwrap();
        let turn_id: String = sqlx::query_scalar("SELECT turn_id FROM task_runs WHERE task_id=? ORDER BY started_at DESC,rowid DESC LIMIT 1").bind(&created.task.id).fetch_one(&server.pool).await.unwrap();
        let turn = turn_id.as_str();
        let row = server.checkpoints.get(turn).await.unwrap().unwrap();
        let usage = crate::task::contract::checkpoint_usage(&row.usage_json).unwrap();
        let known: i64 = sqlx::query_scalar(
            "SELECT SUM(input_tokens+output_tokens) FROM model_requests WHERE turn_id=?",
        )
        .bind(turn)
        .fetch_one(&server.pool)
        .await
        .unwrap();
        assert!(known > 0);
        if fail_accounting {
            assert!(!work.join("report.md").exists());
        } else {
            assert_eq!(known, usage.input_tokens + usage.output_tokens);
            assert_eq!(
                std::fs::read_to_string(work.join("report.md")).unwrap(),
                "Completed before provider failure."
            );
        }
        if fail_accounting {
            assert_eq!(task.status, jeeves_state::TaskStatus::NeedsAttention);
            assert_eq!(row.status, "suspended");
            assert!(task.last_stop_reason.unwrap().contains("checkpoint usage"));
            sqlx::query("DROP TRIGGER fail_round_charge")
                .execute(&server.pool)
                .await
                .unwrap();
        } else {
            assert_eq!(task.status, jeeves_state::TaskStatus::Failed);
            assert_eq!(task.spent_tokens, known);
            assert_eq!(row.status, "abandoned");
        }
        for _ in 0..2 {
            crate::turn::failure::finish(
                &server,
                &sink,
                turn,
                row.run_id.as_deref(),
                Some(&created.task.id),
                "Injected provider failure",
            )
            .await;
        }
        let task = server.tasks.get(&created.task.id).await.unwrap().unwrap();
        assert_eq!(task.spent_tokens, known);
        let run = server.runs.for_turn(turn).await.unwrap().unwrap();
        assert_eq!(run.tokens_in + run.tokens_out, known);
        assert_eq!(
            mock.received.lock().unwrap().len(),
            if fail_accounting { 1 } else { 2 }
        );
    }
}

#[tokio::test]
async fn task_events_replay_a_timeline_from_cursor_zero_in_two_pages() {
    let (server, _home) = boot(None).await;
    let task = create_task(&server, "default", "overnight", None)
        .await
        .task
        .id;
    let other = create_task(&server, "other", "noise", None).await.task.id;

    for (i, run) in ["run-1", "run-1", "run-1", "run-2", "run-2"]
        .iter()
        .enumerate()
    {
        server
            .events
            .append(&task, Some(run), "verify", Some(&format!("e{i}")))
            .await
            .unwrap();
        server
            .events
            .append(&other, None, "verify", Some("noise"))
            .await
            .unwrap();
    }

    let page = |cursor: Option<i32>| {
        let params = json!({ "taskId": task, "cursor": cursor, "limit": 3 });
        async {
            let value = call(&server, jeeves_protocol::methods::TASK_EVENTS, params)
                .await
                .expect("task/events answers");
            serde_json::from_value::<jeeves_protocol::TaskEventsResponse>(value).unwrap()
        }
    };

    let first = page(None).await;
    assert_eq!(first.events.len(), 3);
    assert!(first.has_more, "two more events are waiting");
    let second = page(Some(first.next_cursor)).await;
    assert_eq!(second.events.len(), 2);
    assert!(!second.has_more);

    let all: Vec<_> = first.events.iter().chain(&second.events).collect();
    let details: Vec<_> = all.iter().map(|e| e.detail.clone().unwrap()).collect();
    assert_eq!(details, ["e0", "e1", "e2", "e3", "e4"]);
    assert_eq!(
        all.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert!(
        all.iter().all(|e| e.task_id == task),
        "the other task is filtered out"
    );
    assert_eq!(second.next_cursor, second.latest_cursor);

    let tail = page(Some(second.next_cursor)).await;
    assert!(tail.events.is_empty() && !tail.has_more);
    assert_eq!(tail.next_cursor, second.next_cursor);
}

#[tokio::test]
async fn task_events_at_maximum_page_size_still_reports_more() {
    let (server, _home) = boot(None).await;
    let task = create_task(&server, "default", "large timeline", None)
        .await
        .task
        .id;
    for _ in 0..=jeeves_state::events::MAX_SINCE {
        server
            .events
            .append(&task, None, "verify", None)
            .await
            .unwrap();
    }
    let first = call(
        &server,
        jeeves_protocol::methods::TASK_EVENTS,
        json!({"taskId":task,"cursor":0,"limit":500}),
    )
    .await
    .unwrap();
    let first: jeeves_protocol::TaskEventsResponse = serde_json::from_value(first).unwrap();
    assert_eq!(first.events.len(), 500);
    assert!(first.has_more);
    let second = call(
        &server,
        jeeves_protocol::methods::TASK_EVENTS,
        json!({"taskId":task,"cursor":first.next_cursor,"limit":500}),
    )
    .await
    .unwrap();
    let second: jeeves_protocol::TaskEventsResponse = serde_json::from_value(second).unwrap();
    assert_eq!(second.events.len(), 1);
    assert!(!second.has_more);
    assert_eq!(second.next_cursor, second.latest_cursor);
}

#[tokio::test]
async fn a_task_with_unreadable_stored_checks_is_not_shown_as_unchecked() {
    let (server, _home) = boot(None).await;
    let task = create_task(&server, "task-view-fixture", "read stored checks", None)
        .await
        .task;
    sqlx::query("UPDATE agent_tasks SET checks_json='broken' WHERE id=?")
        .bind(&task.id)
        .execute(&server.pool)
        .await
        .unwrap();
    let reply = call(&server, "task/get", json!({"taskId":task.id})).await;
    assert!(reply.unwrap_err().contains("checks"));
}

#[tokio::test]
async fn task_list_storage_failure_is_reported_and_genuine_absence_stays_empty() {
    let (server, _home) = boot(None).await;
    assert_eq!(
        call(&server, "task/list", json!({})).await.unwrap()["tasks"],
        json!([])
    );
    sqlx::query("ALTER TABLE agent_tasks RENAME TO fixture_unavailable")
        .execute(&server.pool)
        .await
        .unwrap();
    assert!(call(&server, "task/list", json!({})).await.is_err());
}

#[tokio::test]
async fn failed_budget_stop_is_reported_by_the_turn() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("skip"),
        Mock::text("work"),
        Mock::text("more"),
    ]));
    let (server, _home) = boot(Some(mock)).await;
    call(
        &server,
        "task/create",
        json!({"sessionId":"jeeves-main","title":"fixture budget","budget":{"tokens":10}}),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER reject_attention BEFORE UPDATE OF status ON agent_tasks WHEN NEW.status='needs_attention' BEGIN SELECT RAISE(FAIL,'fixture status write failed'); END")
        .execute(&server.pool).await.unwrap();
    let result = call(
        &server,
        "turn/start",
        json!({"sessionId":"jeeves-main","message":"start"}),
    )
    .await;
    assert!(result.unwrap_err().contains("fixture status write failed"));
}

#[tokio::test]
async fn a_task_checks_current_spend_before_admitting_tools() {
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use(
            "first",
            "todo_write",
            json!({"todos":[{"content":"fixture item","status":"in_progress"}]}),
        ),
        Mock::text("an extra call would exceed the budget"),
    ]));
    let (server, _home) = boot(Some(mock.clone())).await;
    let task = call(
        &server,
        "task/create",
        json!({"sessionId":"default","title":"token budget before tools","budget":{"tokens":20}}),
    )
    .await
    .unwrap();
    drive(&server, "default", "start the work", None).await;
    assert_eq!(mock.received.lock().unwrap().len(), 1);
    let view = call(&server, "task/get", json!({"taskId":task["task"]["id"]}))
        .await
        .unwrap();
    assert_eq!(view["task"]["spent"]["tokens"], 28);
    assert_eq!(view["task"]["status"], "needs_attention");
}

#[tokio::test]
async fn an_unchecked_delivery_is_self_checked_against_the_request_before_it_closes() {
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use(
            "call_1",
            "write_file",
            json!({ "path": "notes.md", "content": "Agenda\n" }),
        ),
        Mock::text("written."),
        Mock::tool_use("call_2", "read_file", json!({ "path": "notes.md" })),
        Mock::text("Checked: notes.md holds the agenda."),
    ]));
    let (server, _home) = boot(Some(mock.clone())).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({ "sessionId": "default", "title": "write notes", "cwd": dir.path().display().to_string() }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"].as_str().expect("id").to_string();

    drive(&server, "default", "write the agenda into notes.md", None).await;

    let row = server.tasks.get(&id).await.expect("query").expect("row");
    assert_eq!(
        row.status,
        jeeves_state::TaskStatus::Done,
        "{:?}",
        row.last_stop_reason
    );
    let asked = mock.received.lock().expect("lock").iter().any(|request| {
        request
            .messages
            .iter()
            .any(|m| m.text().contains("Self-check before delivery"))
    });
    assert!(asked, "the self-check round names the original request");
    assert_eq!(row.rounds_used, 2, "the self-check is its own round");
    let events = server.events.list(&id, 20).await.expect("events");
    assert!(
        events.iter().any(|event| event.kind == "self_check"),
        "the self-check is on record"
    );
    let summary: Option<String> = sqlx::query_scalar(
        "SELECT summary FROM task_completions WHERE task_id=? ORDER BY id DESC LIMIT 1",
    )
    .bind(&id)
    .fetch_one(&server.pool)
    .await
    .expect("completion");
    assert_eq!(
        summary.as_deref(),
        Some("Checked: notes.md holds the agenda.")
    );
}

#[tokio::test]
async fn a_correction_that_repeats_the_same_failure_goes_back_to_the_person_early() {
    let draft = |id: &str| {
        Mock::tool_use(
            id,
            "write_file",
            json!({ "path": "notes.md", "content": "draft\n" }),
        )
    };
    let mock = Arc::new(Mock::new(vec![
        draft("call_1"),
        Mock::text("written."),
        draft("call_2"),
        Mock::text("written again."),
        draft("call_3"),
        Mock::text("written a third time."),
    ]));
    let (server, _home) = boot(Some(mock.clone())).await;
    let dir = tempfile::tempdir().expect("temporary directory");
    let created = call(
        &server,
        jeeves_protocol::methods::TASK_CREATE,
        json!({
            "sessionId": "default", "title": "write notes", "maxRounds": 6,
            "cwd": dir.path().display().to_string(),
            "acceptance": [{ "kind": "fileContains", "path": "notes.md", "pattern": "Agenda" }],
        }),
    )
    .await
    .expect("create the task");
    let id = created["task"]["id"].as_str().expect("id").to_string();

    drive(&server, "default", "write the agenda into notes.md", None).await;

    let row = server.tasks.get(&id).await.expect("query").expect("row");
    assert_eq!(row.status, jeeves_state::TaskStatus::NeedsAttention);
    assert!(
        row.suspend_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with("No progress after 3 rounds")),
        "{:?}",
        row.suspend_reason
    );
    assert_eq!(
        row.rounds_used, 3,
        "two repeats of the first failure stop it"
    );
}
