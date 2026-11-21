use crate::{EventSink, Frame, Server};
use jeeves_protocol::{JsonRpcMessage, JsonRpcRequest, RequestId, TurnStartParams};
use jeeves_provider::{CreateRequest, CreateResponse, Provider, ProviderError, Resolved, TextSink};
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

type Reply<'a> = Pin<Box<dyn Future<Output = Result<CreateResponse, ProviderError>> + Send + 'a>>;
#[derive(Default)]
struct BlockingModel {
    entered: Notify,
    release: Notify,
    requests: Mutex<Vec<String>>,
}
impl Provider for BlockingModel {
    fn create(&self, request: CreateRequest) -> Reply<'_> {
        Box::pin(async move {
            let text = request
                .messages
                .iter()
                .map(|m| m.text())
                .collect::<Vec<_>>()
                .join("\n");
            self.requests.lock().unwrap().push(format!(
                "{}\n{}",
                request.system_text().unwrap_or_default(),
                text
            ));
            if text.contains("private-task-marker") {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(jeeves_provider::mock::Mock::text("Task output produced."))
            } else {
                Ok(jeeves_provider::mock::Mock::text("I am here with you."))
            }
        })
    }
    fn stream(&self, request: CreateRequest, _sink: TextSink) -> Reply<'_> {
        self.create(request)
    }
}
async fn rpc(server: &Server, method: &str, params: Value) -> Result<Value, String> {
    let (sink, mut receiver) = EventSink::channel();
    crate::handle(
        server,
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: RequestId::Number(1),
            method: method.into(),
            params: Some(params),
        },
        sink,
    )
    .await;
    while let Some(frame) = receiver.recv().await {
        match frame {
            Frame::Response(JsonRpcMessage::Response(r)) => return Ok(r.result),
            Frame::Response(JsonRpcMessage::Error(r)) => return Err(r.error.message),
            _ => {}
        }
    }
    Err("No response".into())
}
async fn wait_finished(server: &Server, id: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server
                .tasks
                .get(id)
                .await
                .unwrap()
                .unwrap()
                .status
                .is_terminal()
            {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn queued_task_and_conversation_run_independently() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let model = Arc::new(BlockingModel::default());
    server.install_provider(Resolved {
        client: model.clone(),
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    std::fs::write(home.path().join("contract-proof.txt"), "proof").unwrap();
    let created = rpc(
        &server,
        "task/create",
        json!({"title":"Research a private task","sessionId":"jeeves-main","cwd":home.path(),"acceptance":[{"kind":"fileExists","path":"contract-proof.txt"}],"budget":{"tokens":500}}),
    )
    .await
    .unwrap();
    let id = created["task"]["id"].as_str().unwrap();
    rpc(
        &server,
        "task/start",
        json!({"taskId":id,"message":"private-task-marker"}),
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), model.entered.notified())
        .await
        .unwrap();
    assert!(rpc(
        &server,
        "task/start",
        json!({"taskId":id,"message":"duplicate"})
    )
    .await
    .is_err());
    let (sink, mut receiver) = EventSink::channel();
    tokio::time::timeout(
        Duration::from_secs(5),
        crate::run_turn(
            &server,
            TurnStartParams {
                session_id: Some(crate::interaction::MAIN_SESSION.into()),
                message: "I am anxious; keep me company.".into(),
                ..Default::default()
            },
            RequestId::Number(3),
            &sink,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        server.tasks.get(id).await.unwrap().unwrap().status,
        jeeves_state::TaskStatus::Running
    );
    let mut reply = None;
    while let Ok(frame) = receiver.try_recv() {
        if let Frame::Notification(jeeves_protocol::ServerNotification::TurnCompleted(done)) = frame
        {
            reply = Some(done.reply);
        }
    }
    assert_eq!(reply.as_deref(), Some("I am here with you."));
    let task = rpc(&server, "task/get", json!({"taskId":id}))
        .await
        .unwrap();
    assert_ne!(
        task["task"]["executionSessionId"],
        crate::interaction::MAIN_SESSION
    );
    assert!(!model
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("private-task-marker"));
    assert!(model
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("Task progress snapshot"));
    model.release.notify_one();
    wait_finished(&server, id).await;
    assert_eq!(server.runs.list_by_task(id, 10).await.unwrap().len(), 1);
    let requests = model.requests.lock().unwrap();
    assert!(requests[0].contains("Durable task handoff"));
    assert!(requests[0].contains("contract-proof.txt"));
    assert!(!requests.last().unwrap().contains("contract-proof.txt"));
}

#[tokio::test]
async fn recovery_waits_for_a_live_channel_and_never_resubmits_claimed_work() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("state.db");
    let pool = jeeves_state::open(&path).await.unwrap();
    let tasks = jeeves_state::Tasks::new(pool.clone());
    for id in ["pending-task", "unknown-task"] {
        tasks
            .create(jeeves_state::NewTask {
                id,
                session_id: Some("chat"),
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
    }
    let queue = jeeves_state::dispatch::TaskDispatch::new(pool.clone());
    queue
        .enqueue("unknown-task", "must not repeat")
        .await
        .unwrap();
    queue.claim().await.unwrap();
    queue
        .enqueue("pending-task", "authorized work")
        .await
        .unwrap();
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let mock = Arc::new(jeeves_provider::mock::Mock::new(vec![
        jeeves_provider::mock::Mock::text("Produced."),
    ]));
    server.install_provider(Resolved {
        client: mock.clone(),
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    assert_eq!(
        server
            .tasks
            .get("pending-task")
            .await
            .unwrap()
            .unwrap()
            .status,
        jeeves_state::TaskStatus::Pending
    );
    let (sink, mut events) = EventSink::channel();
    super::dispatch::kick(server.clone(), sink);
    wait_finished(&server, "pending-task").await;
    let mut started = false;
    while let Ok(frame) = events.try_recv() {
        started |= matches!(
            frame,
            Frame::Notification(jeeves_protocol::ServerNotification::TurnStarted(_))
        );
    }
    assert!(
        started,
        "recovered work must have an observable event channel"
    );
    assert_eq!(
        server
            .tasks
            .get("unknown-task")
            .await
            .unwrap()
            .unwrap()
            .status,
        jeeves_state::TaskStatus::NeedsAttention
    );
    assert_eq!(
        server
            .runs
            .list_by_task("unknown-task", 10)
            .await
            .unwrap()
            .len(),
        0
    );
    assert_eq!(mock.received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_queued_pause_survives_until_explicit_resume() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let model = Arc::new(jeeves_provider::mock::Mock::new(vec![
        jeeves_provider::mock::Mock::text("Produced."),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    let lock = server.session_write_lock("background-task-worker");
    let guard = lock.lock().await;
    let created = rpc(
        &server,
        "task/create",
        json!({"title":"pause before starting","sessionId":"jeeves-main"}),
    )
    .await
    .unwrap();
    let id = created["task"]["id"].as_str().unwrap();
    rpc(
        &server,
        "task/start",
        json!({"taskId":id,"message":"authorized work"}),
    )
    .await
    .unwrap();
    rpc(&server, "task/suspend", json!({"taskId":id}))
        .await
        .unwrap();
    assert!(model.received.lock().unwrap().is_empty());
    let resumed = rpc(&server, "task/resume", json!({"taskId":id}))
        .await
        .unwrap();
    assert_eq!(resumed["task"]["status"], "pending");
    drop(guard);
    wait_finished(&server, id).await;
    assert_eq!(server.runs.list_by_task(id, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn unavailable_task_ledger_stops_before_any_model_request() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = Server::boot(
        pool.clone(),
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let mock = Arc::new(jeeves_provider::mock::Mock::new(vec![]));
    server.install_provider(Resolved {
        client: mock.clone(),
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    sqlx::query("ALTER TABLE agent_tasks RENAME TO unavailable_tasks")
        .execute(&pool)
        .await
        .unwrap();
    let (sink, _receiver) = EventSink::channel();
    let error = crate::run_turn(
        &server,
        TurnStartParams {
            message: "do work".into(),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("Cannot read task state"));
    assert!(mock.received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn human_control_wins_while_recovery_waits_for_the_worker() {
    for action in ["task/suspend", "task/cancel"] {
        let home = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap();
        let server = Server::boot(
            pool.clone(),
            jeeves_config::Settings {
                home: home.path().into(),
                ..Default::default()
            },
        )
        .await;
        let mock = Arc::new(jeeves_provider::mock::Mock::new(vec![]));
        server.install_provider(Resolved {
            client: mock.clone(),
            model: "test".into(),
            small_model: "test".into(),
            provider_id: "mock".into(),
            vision: false,
            context_window: 100_000,
        });
        let lock = server.session_write_lock("background-task-worker");
        let guard = lock.lock().await;
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: "recover",
                session_id: Some("chat"),
                title: "recover",
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
        let queue = jeeves_state::dispatch::TaskDispatch::new(pool);
        let row = queue.enqueue("recover", "work").await.unwrap();
        queue.claim().await.unwrap();
        queue.finish("recover").await.unwrap();
        server
            .runs
            .start(
                &row.turn_id,
                "recover",
                Some(&row.session_id),
                Some(&row.turn_id),
            )
            .await
            .unwrap();
        server
            .tasks
            .set_running("recover", &row.turn_id)
            .await
            .unwrap();
        server
            .checkpoints
            .save_round(
                &row.turn_id,
                &row.session_id,
                "work",
                "[]",
                0,
                "{}",
                Some(&row.turn_id),
                Some("round"),
            )
            .await
            .unwrap();
        let (sink, mut receiver) = EventSink::channel();
        let checkpoint = server.checkpoints.get(&row.turn_id).await.unwrap().unwrap();
        let owners = Arc::strong_count(&lock);
        let mut recovery = Box::pin(crate::resume::resume_turn(&server, &sink, checkpoint, None));
        tokio::select! {
            result = &mut recovery => panic!("Recovery completed while worker was held: {result}"),
            result = tokio::time::timeout(Duration::from_secs(5), async {
                while Arc::strong_count(&lock) == owners { tokio::task::yield_now().await; }
            }) => result.expect("recovery did not reach the worker lock"),
        }
        rpc(&server, action, json!({"taskId":"recover"}))
            .await
            .unwrap();
        drop(guard);
        assert!(!tokio::time::timeout(Duration::from_secs(5), recovery)
            .await
            .unwrap());
        while let Ok(frame) = receiver.try_recv() {
            assert!(!matches!(
                frame,
                Frame::Notification(
                    jeeves_protocol::ServerNotification::TurnStarted(_)
                        | jeeves_protocol::ServerNotification::TurnResumed(_)
                )
            ));
        }
        assert!(mock.received.lock().unwrap().is_empty());
        let task = server.tasks.get("recover").await.unwrap().unwrap();
        assert_eq!(
            task.status,
            if action == "task/cancel" {
                jeeves_state::TaskStatus::Cancelled
            } else {
                jeeves_state::TaskStatus::Suspended
            }
        );
    }
}

async fn boot_with_workers(workers: i32, replies: usize) -> (tempfile::TempDir, Server) {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().into(),
            task_workers: workers,
            ..Default::default()
        },
    )
    .await;
    let model = Arc::new(jeeves_provider::mock::Mock::new(
        (0..replies)
            .map(|_| jeeves_provider::mock::Mock::text("Produced."))
            .collect(),
    ));
    server.install_provider(Resolved {
        client: model,
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    (home, server)
}

#[tokio::test]
async fn a_second_worker_runs_a_task_while_the_first_slot_is_busy() {
    let (_home, server) = boot_with_workers(2, 2).await;
    let slot0 = server.session_write_lock(&super::dispatch::slot_lock_name(0));
    let _busy = slot0.lock().await;
    let created = rpc(
        &server,
        "task/create",
        json!({"title":"parallel","sessionId":"jeeves-main"}),
    )
    .await
    .unwrap();
    let id = created["task"]["id"].as_str().unwrap().to_string();
    rpc(&server, "task/start", json!({"taskId":id,"message":"work"}))
        .await
        .unwrap();
    wait_finished(&server, &id).await;
    assert_eq!(server.runs.list_by_task(&id, 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn one_worker_leaves_the_second_task_waiting() {
    let (_home, server) = boot_with_workers(1, 2).await;
    let slot0 = server.session_write_lock(&super::dispatch::slot_lock_name(0));
    let busy = slot0.lock().await;
    let created = rpc(
        &server,
        "task/create",
        json!({"title":"waits","sessionId":"jeeves-main"}),
    )
    .await
    .unwrap();
    let id = created["task"]["id"].as_str().unwrap().to_string();
    rpc(&server, "task/start", json!({"taskId":id,"message":"work"}))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!server
        .tasks
        .get(&id)
        .await
        .unwrap()
        .unwrap()
        .status
        .is_terminal());
    drop(busy);
    wait_finished(&server, &id).await;
}

#[tokio::test]
async fn an_expired_lease_without_a_checkpoint_goes_to_review_once() {
    let (_home, server) = boot_with_workers(2, 0).await;
    server
        .tasks
        .create(jeeves_state::NewTask {
            id: "stuck",
            session_id: Some("chat"),
            title: "stuck",
            goal_condition: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let queue = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    queue.enqueue("stuck", "work").await.unwrap();
    queue
        .claim_leased("worker-9:1", -1, 2)
        .await
        .unwrap()
        .unwrap();
    let (sink, _events) = EventSink::channel();
    super::dispatch::sweep(&server, &sink).await;
    super::dispatch::sweep(&server, &sink).await;
    let task = server.tasks.get("stuck").await.unwrap().unwrap();
    assert_eq!(task.status, jeeves_state::TaskStatus::NeedsAttention);
    let expiries = server
        .events
        .list("stuck", 50)
        .await
        .unwrap()
        .iter()
        .filter(|event| event.kind == "lease_expired")
        .count();
    assert_eq!(expiries, 1, "swept once, announced once");
}

#[tokio::test]
async fn a_held_task_is_not_resumed_by_crash_recovery() {
    let (_home, server) = boot_with_workers(1, 0).await;
    server
        .tasks
        .create(jeeves_state::NewTask {
            id: "held",
            session_id: Some("chat"),
            title: "held",
            goal_condition: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let queue = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    let row = queue.enqueue("held", "work").await.unwrap();
    queue.claim().await.unwrap();
    queue.finish("held").await.unwrap();
    server
        .runs
        .start(
            &row.turn_id,
            "held",
            Some(&row.session_id),
            Some(&row.turn_id),
        )
        .await
        .unwrap();
    server
        .tasks
        .set_running("held", &row.turn_id)
        .await
        .unwrap();
    server
        .checkpoints
        .save_round(
            &row.turn_id,
            &row.session_id,
            "work",
            "[]",
            0,
            "{}",
            Some(&row.turn_id),
            Some("round"),
        )
        .await
        .unwrap();
    sqlx::query("INSERT INTO browser_holds(task_id,url) VALUES('held','https://example.com')")
        .execute(&server.pool)
        .await
        .unwrap();
    let (sink, _events) = EventSink::channel();
    crate::resume::reconcile(&server, &sink).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let status = server
                .checkpoints
                .get(&row.turn_id)
                .await
                .unwrap()
                .unwrap()
                .status;
            if status == "suspended" {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a held task must end suspended, not resume behind the human's back");
}
