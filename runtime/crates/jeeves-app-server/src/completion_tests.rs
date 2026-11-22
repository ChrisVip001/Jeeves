use crate::{EventSink, Server};
use jeeves_protocol::{RequestId, TurnStartParams};
use jeeves_provider::{mock::Mock, Resolved};
use jeeves_tools::notify::{Notifier, NotifyEvent};
use std::sync::{Arc, Mutex};

struct Observer {
    pool: sqlx::SqlitePool,
    task_id: String,
    statuses: Mutex<Vec<String>>,
}
impl Notifier for Observer {
    fn notify(&self, _event: NotifyEvent) {
        let status = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(
                sqlx::query_scalar::<_, String>("SELECT status FROM agent_tasks WHERE id=?")
                    .bind(&self.task_id)
                    .fetch_one(&self.pool),
            )
        })
        .unwrap();
        self.statuses.lock().unwrap().push(status);
    }
    fn delivery_target(&self) -> Option<String> {
        Some("test-target".into())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_completion_notice_observes_the_committed_task_outcome() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let mut server = Server::boot(
        pool.clone(),
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let mock = Arc::new(Mock::new(vec![
        Mock::text("Produced."),
        Mock::text(r#"{"ok":true,"impossible":false,"reason":"measured"}"#),
    ]));
    server.install_provider(Resolved {
        client: mock,
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    let task = server
        .set_task_goal("chat", Some("finish the authorized work"))
        .await
        .unwrap();
    let observer = Arc::new(Observer {
        pool,
        task_id: task.id,
        statuses: Mutex::new(vec![]),
    });
    server.notifier = Some(observer.clone());
    let (sink, _receiver) = EventSink::channel();
    crate::run_turn(
        &server,
        TurnStartParams {
            session_id: Some("chat".into()),
            message: "do the work".into(),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap();
    let statuses = observer.statuses.lock().unwrap();
    assert_eq!(
        *statuses,
        vec!["done"],
        "No completion may be advertised before its task outcome commits"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_completion_commit_cannot_return_a_successful_turn() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let mut server = Server::boot(
        pool.clone(),
        jeeves_config::Settings {
            home: home.path().into(),
            ..Default::default()
        },
    )
    .await;
    let mock = Arc::new(Mock::new(vec![
        Mock::text("Produced."),
        Mock::text(r#"{"ok":true,"impossible":false,"reason":"measured"}"#),
    ]));
    server.install_provider(Resolved {
        client: mock,
        model: "test".into(),
        small_model: "test".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    let task = server
        .set_task_goal("chat", Some("finish the authorized work"))
        .await
        .unwrap();
    let observer = Arc::new(Observer {
        pool: pool.clone(),
        task_id: task.id.clone(),
        statuses: Mutex::new(vec![]),
    });
    server.notifier = Some(observer.clone());
    sqlx::query("CREATE TRIGGER fail_done BEFORE INSERT ON task_completions WHEN NEW.status='done' BEGIN SELECT RAISE(FAIL,'injected completion failure'); END").execute(&pool).await.unwrap();
    let (sink, _receiver) = EventSink::channel();
    let error = crate::run_turn(
        &server,
        TurnStartParams {
            session_id: Some("chat".into()),
            message: "do the work".into(),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("completion is not confirmed"));
    assert_eq!(
        server.tasks.get(&task.id).await.unwrap().unwrap().status,
        jeeves_state::TaskStatus::NeedsAttention
    );
    assert!(observer.statuses.lock().unwrap().is_empty());
    let notices = server.completions.pending().await.unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].status, "needs_attention");
}

struct QueueSink {
    target: String,
    fail: bool,
    seen: Mutex<Vec<String>>,
}
impl Notifier for QueueSink {
    fn notify(&self, _event: NotifyEvent) {
        panic!("Acknowledged enqueue is required");
    }
    fn delivery_target(&self) -> Option<String> {
        Some(self.target.clone())
    }
    fn enqueue(&self, event: NotifyEvent) -> Result<(), String> {
        if self.fail {
            return Err("Injected enqueue failure".into());
        }
        self.seen.lock().unwrap().push(event.message());
        Ok(())
    }
}
#[tokio::test]
async fn task_delivery_survives_enqueue_failure_and_destination_changes() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let tasks = jeeves_state::Tasks::new(pool.clone());
    tasks
        .create(jeeves_state::NewTask {
            id: "task",
            session_id: Some("chat"),
            title: "work",
            goal_condition: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let rows = jeeves_state::completions::Completions::new(pool.clone());
    rows.bind("task", true, Some("original")).await.unwrap();
    tasks
        .finish("task", jeeves_state::TaskStatus::Done)
        .await
        .unwrap();
    crate::completion_delivery::deliver(
        &rows,
        &QueueSink {
            target: "original".into(),
            fail: true,
            seen: Mutex::new(vec![]),
        },
    )
    .await;
    assert_eq!(rows.pending().await.unwrap().len(), 1);
    drop(rows);
    let rows = jeeves_state::completions::Completions::new(pool);
    let changed = QueueSink {
        target: "changed".into(),
        fail: false,
        seen: Mutex::new(vec![]),
    };
    crate::completion_delivery::deliver(&rows, &changed).await;
    assert!(changed.seen.lock().unwrap().is_empty());
    let accepted = QueueSink {
        target: "original".into(),
        fail: false,
        seen: Mutex::new(vec![]),
    };
    crate::completion_delivery::deliver(&rows, &accepted).await;
    crate::completion_delivery::deliver(&rows, &accepted).await;
    assert_eq!(accepted.seen.lock().unwrap().len(), 1);
    assert!(rows.pending().await.unwrap().is_empty());
}

#[tokio::test]
async fn schedule_notification_retries_the_original_committed_destination() {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let schedules = jeeves_state::Schedules::new(pool.clone());
    let rows = jeeves_state::completions::Completions::new(pool);
    schedules.ensure("job", true, "notify").await.unwrap();
    schedules.claim("job", "minute").await.unwrap();
    rows.schedules()
        .settle(jeeves_state::schedule_completions::ScheduleCompletion {
            name: "job",
            minute: "minute",
            status: "needs_attention",
            detail: "Review required",
            output: "",
            preview: "Unknown effects",
            output_path: None,
            notify_requested: true,
            notify_target: Some("original"),
        })
        .await
        .unwrap();
    crate::completion_delivery::deliver(
        &rows,
        &QueueSink {
            target: "original".into(),
            fail: true,
            seen: Mutex::new(vec![]),
        },
    )
    .await;
    assert_eq!(rows.schedules().pending().await.unwrap().len(), 1);
    let changed = QueueSink {
        target: "changed".into(),
        fail: false,
        seen: Mutex::new(vec![]),
    };
    crate::completion_delivery::deliver(&rows, &changed).await;
    assert!(changed.seen.lock().unwrap().is_empty());
    let accepted = QueueSink {
        target: "original".into(),
        fail: false,
        seen: Mutex::new(vec![]),
    };
    crate::completion_delivery::deliver(&rows, &accepted).await;
    crate::completion_delivery::deliver(&rows, &accepted).await;
    assert_eq!(accepted.seen.lock().unwrap().len(), 1);
    assert!(rows.schedules().pending().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_crash_after_local_enqueue_reuses_the_same_queue_file_after_restart() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("state.db");
    let pool = jeeves_state::open(&db).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/fixture", listener.local_addr().unwrap());
    std::fs::write(
        home.path().join("notify.json"),
        serde_json::json!({"slack":url}).to_string(),
    )
    .unwrap();
    let notifier = jeeves_tools::notify::load(home.path(), true).unwrap();
    let target = notifier.delivery_target().unwrap();
    let tasks = jeeves_state::Tasks::new(pool.clone());
    tasks
        .create(jeeves_state::NewTask {
            id: "fixture",
            session_id: Some("main"),
            title: "Fixture",
            goal_condition: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let rows = jeeves_state::completions::Completions::new(pool.clone());
    rows.bind("fixture", true, Some(&target)).await.unwrap();
    tasks
        .finish("fixture", jeeves_state::TaskStatus::Done)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_mark BEFORE UPDATE OF enqueued_at ON task_completions BEGIN SELECT RAISE(ABORT,'fixture crash before marker'); END").execute(&pool).await.unwrap();
    crate::completion_delivery::deliver(&rows, notifier.as_ref()).await;
    assert_eq!(rows.pending().await.unwrap().len(), 1);
    pool.close().await;
    let reopened = jeeves_state::open(&db).await.unwrap();
    sqlx::query("DROP TRIGGER fail_mark")
        .execute(&reopened)
        .await
        .unwrap();
    let rows = jeeves_state::completions::Completions::new(reopened);
    let restarted = jeeves_tools::notify::load(home.path(), true).unwrap();
    crate::completion_delivery::deliver(&rows, restarted.as_ref()).await;
    assert!(rows.pending().await.unwrap().is_empty());
    let dir = home.path().join("outbox/notifications").join(target);
    assert_eq!(
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .count(),
        1
    );
}
