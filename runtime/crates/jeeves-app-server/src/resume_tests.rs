//! Startup reconciliation tests — the crash-recovery half answers one question:
//! **does a fresh process actually continue, and does it continue honestly**.
//!
//! The "previous process" is simulated by writing a checkpoint row the way the checkpoint
//! outlet would have left it after a kill -9 mid-turn: `open`, working memory serialized.

use std::sync::Arc;
use std::time::Duration;

use jeeves_config::Settings;
use jeeves_protocol::ServerNotification;
use jeeves_provider::mock::Mock;
use jeeves_provider::{ContentBlock, Message, Resolved, Role};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{resume, EventSink, Frame, Server};

async fn server_with_path(mock: Option<Arc<Mock>>) -> (Server, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let settings = Settings {
        home: home.clone(),
        ..Settings::default()
    };
    let _ = dir.keep();
    let server = Server::boot(pool, settings).await;
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

async fn server(mock: Option<Arc<Mock>>) -> Server {
    server_with_path(mock).await.0
}

async fn back_date(home: &std::path::Path, turn_id: &str, modifier: &str) {
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    sqlx::query("UPDATE turn_checkpoints SET updated_at = datetime('now', ?) WHERE turn_id = ?")
        .bind(modifier)
        .bind(turn_id)
        .execute(&pool)
        .await
        .expect("change the timestamp");
}

fn crashed_working_memory() -> String {
    serde_json::to_string(&vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "watch the server for me for three days".to_string(),
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "memory".to_string(),
                input: json!({"action": "save", "subject": "the server", "content": "watch it for three days"}),
                extra: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".to_string(),
                content: "noted".to_string(),
            }],
        },
    ])
    .expect("serialize")
}

#[tokio::test]
async fn an_open_checkpoint_resumes_and_gets_full_accounting() {
    let mock = Arc::new(Mock::new(vec![Mock::text(
        "carrying on from where it stopped, the watching is done.",
    )]));
    let server = server(Some(mock.clone())).await;

    server
        .checkpoints
        .save(
            "t-crash",
            "test",
            "watch the server for me for three days",
            &crashed_working_memory(),
            2,
            r#"{"input_tokens":100,"output_tokens":20}"#,
            None,
        )
        .await
        .expect("write the checkpoint");

    let (tx, mut rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;

    let mut notifications: Vec<ServerNotification> = Vec::new();
    let mut completed = false;
    for _ in 0..200 {
        while let Ok(frame) = rx.try_recv() {
            if let Frame::Notification(n) = frame {
                completed |= matches!(n, ServerNotification::TurnCompleted(_));
                notifications.push(n);
            }
        }
        if completed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(completed, "the resume did not run through");

    assert!(matches!(
        notifications.first(),
        Some(ServerNotification::TurnStarted(started))
            if started.turn_id == "t-crash" && started.user_message == "watch the server for me for three days"
    ));
    assert!(notifications.iter().any(|n| matches!(
        n,
        ServerNotification::TurnResumed(resumed)
            if resumed.from_iterations == 2 && resumed.reason == "reconcile: crash recovery"
    )));

    let (calls, message_count) = {
        let received = mock.received.lock().expect("lock");
        (received.len(), received.first().map(|r| r.messages.len()))
    };
    assert_eq!(calls, 1);
    assert_eq!(
        message_count,
        Some(3),
        "the request carries the checkpoint's working memory"
    );

    let history = server
        .chat
        .session_history("test")
        .await
        .expect("read the conversation log");
    assert_eq!(
        history.len(),
        1,
        "a resumed turn lands in the chat log as usual"
    );
    assert!(history[0].1.contains("the watching is done"));
    assert!(
        server
            .checkpoints
            .open_rows()
            .await
            .expect("read the register")
            .is_empty(),
        "a resume closes as soon as it is done"
    );
}

#[tokio::test]
async fn without_a_model_the_checkpoint_stays_open_for_later() {
    let server = server(None).await;
    server
        .checkpoints
        .save("t-wait", "test", "a long task", "[]", 1, "{}", None)
        .await
        .expect("write the checkpoint");

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        server
            .checkpoints
            .open_rows()
            .await
            .expect("read the register")
            .len(),
        1,
        "the model is not configured: keep it for now and resume once it is"
    );
}

#[tokio::test]
async fn unreadable_working_memory_is_abandoned_not_resumed() {
    let mock = Arc::new(Mock::new(vec![Mock::text("whatever")]));
    let server = server(Some(mock)).await;
    server
        .checkpoints
        .save(
            "t-broken",
            "test",
            "a long task",
            "not json at all",
            1,
            "{}",
            None,
        )
        .await
        .expect("write the checkpoint");

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    for _ in 0..100 {
        if server
            .checkpoints
            .open_rows()
            .await
            .unwrap_or_default()
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(server
        .checkpoints
        .open_rows()
        .await
        .expect("read the register")
        .is_empty());
}

#[tokio::test]
async fn a_turn_error_marks_its_checkpoint_abandoned() {
    let mock = Arc::new(Mock::new(vec![]));
    let server = server(Some(mock)).await;
    server
        .checkpoints
        .save(
            "t-doomed",
            "test",
            "a task that is bound to fail",
            "[]",
            1,
            "{}",
            None,
        )
        .await
        .expect("write the checkpoint");

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    for _ in 0..100 {
        if server
            .checkpoints
            .open_rows()
            .await
            .unwrap_or_default()
            .is_empty()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(server
        .checkpoints
        .open_rows()
        .await
        .expect("read the register")
        .is_empty());
}

#[tokio::test]
async fn a_checkpoint_past_the_age_limit_is_abandoned_not_resumed() {
    let (server, home) = server_with_path(None).await;
    server
        .checkpoints
        .save(
            "t-stale",
            "test",
            "give me a briefing on today's tech news",
            "[]",
            1,
            "{}",
            None,
        )
        .await
        .expect("write the checkpoint");
    back_date(&home, "t-stale", "-2 days").await;

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let row = server
        .checkpoints
        .get("t-stale")
        .await
        .expect("read")
        .expect("present");
    assert_eq!(
        row.status, "abandoned",
        "an over-age turn must not be resumed automatically"
    );
    assert!(
        server
            .checkpoints
            .open_rows()
            .await
            .expect("read")
            .is_empty(),
        "once marked abandoned there is nothing left to resume — the next boot must not print that line again"
    );
}

#[tokio::test]
async fn a_zero_limit_keeps_the_old_resume_everything_behaviour() {
    let (server, home) = server_with_path(None).await;
    server.set_settings(Settings {
        home: home.clone(),
        resume_max_age_minutes: 0,
        ..Settings::default()
    });
    server
        .checkpoints
        .save(
            "t-old",
            "test",
            "something from two days ago",
            "[]",
            1,
            "{}",
            None,
        )
        .await
        .expect("write the checkpoint");
    back_date(&home, "t-old", "-2 days").await;

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let row = server
        .checkpoints
        .get("t-old")
        .await
        .expect("read")
        .expect("present");
    assert_eq!(row.status, "open", "0 means no limit, not never resume");
}

#[test]
fn an_unreadable_stamp_is_not_evidence_of_age() {
    let now = chrono::NaiveDateTime::parse_from_str("2026-11-12 06:00:00", "%Y-%m-%d %H:%M:%S")
        .expect("parse");
    assert_eq!(
        super::resume::age_minutes("not a time", now),
        None,
        "an unreadable timestamp is unknown"
    );
    assert_eq!(
        super::resume::age_minutes("2026-11-12 05:30:00", now),
        Some(30)
    );
    assert_eq!(
        super::resume::age_minutes("2026-11-12 07:00:00", now),
        Some(-60)
    );
}

#[test]
fn ages_read_as_minutes_hours_and_days() {
    assert_eq!(super::resume::humanize(0), "0 minutes");
    assert_eq!(super::resume::humanize(59), "59 minutes");
    assert_eq!(super::resume::humanize(60), "1 hours");
    assert_eq!(super::resume::humanize(119), "1 hours");
    assert_eq!(super::resume::humanize(1_439), "23 hours");
    assert_eq!(super::resume::humanize(1_440), "1 days");
    assert_eq!(super::resume::humanize(2_880), "2 days");
}

#[tokio::test]
async fn recovered_task_must_pass_acceptance_before_done() {
    for (file_exists, budget, goal, expected) in [
        (false, "{}", None, jeeves_state::TaskStatus::NeedsAttention),
        (true, "{}", None, jeeves_state::TaskStatus::Done),
        (
            true,
            r#"{"tokens":50}"#,
            None,
            jeeves_state::TaskStatus::NeedsAttention,
        ),
        (
            true,
            "{}",
            Some("confirm the report's quality"),
            jeeves_state::TaskStatus::NeedsAttention,
        ),
    ] {
        let mock = Arc::new(Mock::new(vec![Mock::text("the report is finished.")]));
        let (server, home) = server_with_path(Some(mock.clone())).await;
        if file_exists {
            std::fs::write(home.join("report.md"), "the report").expect("the file");
        }
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: "recovered-report",
                session_id: Some("test"),
                title: "write the report",
                goal_condition: goal,
                acceptance_json: r#"[{"kind":"fileExists","path":"report.md"}]"#,
                budget_json: budget,
                cwd: home.to_str(),
                max_rounds: 1,
                tier: None,
            })
            .await
            .expect("the task");
        server
            .runs
            .start(
                "t-report",
                "recovered-report",
                Some("test"),
                Some("t-report"),
            )
            .await
            .expect("the run record");
        server
            .checkpoints
            .save_round(
                "t-report",
                "test",
                "write the report",
                "[]",
                1,
                r#"{"input_tokens":90,"output_tokens":0}"#,
                Some("t-report"),
                Some("report-round"),
            )
            .await
            .expect("the checkpoint");
        server
            .runs
            .account_round(
                "t-report",
                "report-round",
                &jeeves_state::runs::RoundAccount {
                    tokens_in: 100,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        resume::reconcile(&server, &EventSink::new(tx)).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(frame) = rx.recv().await {
                if matches!(&frame, Frame::Notification(ServerNotification::TaskUpdated(update))
                    if budget != "{}" && update.status == "needs_attention")
                    || matches!(
                        frame,
                        Frame::Notification(ServerNotification::TurnCompleted(_))
                    )
                {
                    return;
                }
            }
            panic!("not finished");
        })
        .await
        .expect("the recovery times out");
        let task = server
            .tasks
            .get("recovered-report")
            .await
            .expect("query")
            .expect("it exists");
        assert_eq!(task.status, expected);
        if budget == "{}" {
            assert_eq!(task.spent_tokens, 115);
            assert!(task.checks_json.contains("report.md"));
        } else {
            assert_eq!(task.spent_tokens, 100);
            assert!(mock.received.lock().unwrap().is_empty());
        }
        assert_eq!(task.spent_tools, 0);
    }
}

#[tokio::test]
async fn pending_tool_intent_is_not_replayed() {
    let mock = Arc::new(Mock::new(vec![Mock::text("must not run")]));
    let server = server(Some(mock.clone())).await;
    let mut messages: Vec<Message> = serde_json::from_str(&crashed_working_memory()).unwrap();
    messages.pop();
    server
        .checkpoints
        .save(
            "unknown",
            "test",
            "work",
            &serde_json::to_string(&messages).unwrap(),
            0,
            "{}",
            None,
        )
        .await
        .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if server
                .checkpoints
                .get("unknown")
                .await
                .unwrap()
                .unwrap()
                .status
                == "suspended"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(mock.received.lock().unwrap().is_empty());
}

#[tokio::test]
async fn recovered_goal_uses_the_shared_driver_until_satisfied() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("First half complete."),
        Mock::text(r#"{"ok":false,"impossible":false,"reason":"finish the remaining half"}"#),
        Mock::text("Both halves complete."),
        Mock::text(r#"{"ok":true,"impossible":false,"reason":"verified"}"#),
    ]));
    let (server, _) = server_with_path(Some(mock.clone())).await;
    server
        .tasks
        .create(jeeves_state::NewTask {
            id: "resume-goal",
            session_id: Some("test"),
            title: "two halves",
            goal_condition: Some("finish both halves"),
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 2,
            tier: None,
        })
        .await
        .unwrap();
    server
        .runs
        .start("goal-run", "resume-goal", Some("test"), Some("goal-run"))
        .await
        .unwrap();
    server
        .checkpoints
        .save_round(
            "goal-run",
            "test",
            "start",
            "[]",
            1,
            r#"{"input_tokens":0,"output_tokens":0}"#,
            Some("goal-run"),
            Some("goal-round-1"),
        )
        .await
        .unwrap();
    jeeves_agent::plan::record(
        &server.steps,
        "resume-goal",
        &jeeves_agent::plan::Plan {
            version: 2,
            steps: vec!["Use the restored evidence ledger".into()],
        },
    )
    .await
    .unwrap();
    let (sink, mut receiver) = EventSink::channel();
    resume::reconcile(&server, &sink).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = receiver.recv().await {
            if let Frame::Notification(ServerNotification::TurnCompleted(done)) = frame {
                assert_eq!(done.meta.goal_status.as_deref(), Some("satisfied"));
                assert_eq!(done.meta.goal_rounds, 2);
                return;
            }
        }
        panic!("recovered goal never completed");
    })
    .await
    .unwrap();
    let task = server.tasks.get("resume-goal").await.unwrap().unwrap();
    assert_eq!(task.status, jeeves_state::TaskStatus::Done);
    assert_eq!(task.rounds_used, 2);
    assert_eq!(
        server.runs.get("goal-run").await.unwrap().unwrap().rounds,
        2
    );
    let requests = mock.received.lock().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(requests[0]
        .system_text()
        .unwrap()
        .contains("finish both halves"));
    assert!(requests[2]
        .system_text()
        .unwrap()
        .contains("finish both halves"));

    assert!(
        requests[2]
            .messages
            .iter()
            .any(|m| m.text().contains("Use the restored evidence ledger")),
        "goal continuation must retain the persisted plan"
    );

    assert!(requests.iter().any(|request| request
        .messages
        .iter()
        .any(|m| m.text().contains("finish the remaining half"))));
}

#[tokio::test]
async fn recovery_uses_the_checkpoint_round_cap_instead_of_a_stale_task_counter() {
    let mock = Arc::new(Mock::new(vec![
        Mock::text("Second round result."),
        Mock::text(r#"{"ok":false,"impossible":false,"reason":"still incomplete"}"#),
    ]));
    let (server, _) = server_with_path(Some(mock.clone())).await;
    server
        .tasks
        .create(jeeves_state::NewTask {
            id: "round-cap",
            session_id: Some("test"),
            title: "work",
            goal_condition: Some("finish work"),
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 2,
            tier: None,
        })
        .await
        .unwrap();
    server
        .runs
        .start("round-run", "round-cap", Some("test"), Some("round-turn"))
        .await
        .unwrap();
    server
        .checkpoints
        .save_numbered_round(jeeves_state::checkpoints::CheckpointWrite {
            turn_id: "round-turn",
            session_id: "test",
            user_message: "continue work",
            messages_json: "[]",
            iterations: 0,
            usage_json: "{}",
            run_id: Some("round-run"),
            round_id: Some("second-round"),
            round_no: Some(2),
        })
        .await
        .unwrap();
    server.tasks.record_round("round-cap", 1).await.unwrap();
    let (sink, mut receiver) = EventSink::channel();
    resume::reconcile(&server, &sink).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = receiver.recv().await {
            if let Frame::Notification(ServerNotification::TurnCompleted(done)) = frame {
                assert_eq!(done.meta.goal_rounds, 2);
                assert_eq!(done.meta.goal_status.as_deref(), Some("round-limit"));
                return;
            }
        }
        panic!("recovery never completed");
    })
    .await
    .unwrap();
    assert_eq!(mock.received.lock().unwrap().len(), 2);
    assert_eq!(
        server
            .tasks
            .get("round-cap")
            .await
            .unwrap()
            .unwrap()
            .rounds_used,
        2
    );
}

fn cut_off(calls: &[(&str, &str, serde_json::Value)]) -> String {
    serde_json::to_string(&vec![
        Message::user_text("look into the logs"),
        Message {
            role: Role::Assistant,
            content: calls
                .iter()
                .map(|(id, name, input)| ContentBlock::ToolUse {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                    extra: None,
                })
                .collect(),
        },
    ])
    .unwrap()
}

async fn status_of(server: &Server, turn: &str, want: &str) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while server.checkpoints.get(turn).await.unwrap().unwrap().status != want {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("checkpoint {turn} never became {want}"));
}

#[tokio::test]
async fn an_interrupted_read_is_reported_to_the_model_and_the_turn_continues() {
    let mock = Arc::new(Mock::new(vec![Mock::text("read it again, all good")]));
    let server = server(Some(mock.clone())).await;
    let memory = cut_off(&[
        ("r1", "read_file", json!({"path": "/tmp/app.log"})),
        ("r2", "memory", json!({"action": "search", "query": "logs"})),
    ]);
    let round = "round-1";
    server
        .checkpoints
        .save_with_operations(
            jeeves_state::checkpoints::CheckpointWrite {
                turn_id: "t-read",
                session_id: "test",
                user_message: "look into the logs",
                messages_json: &memory,
                iterations: 1,
                usage_json: "{}",
                run_id: None,
                round_id: Some(round),
                round_no: Some(1),
            },
            &[
                jeeves_state::operations::OperationSnapshot {
                    proposed_only: false,
                    tool_use_id: "r1".into(),
                    tool: "read_file".into(),
                    input_json: json!({"path": "/tmp/app.log"}).to_string(),
                    output: None,
                },
                jeeves_state::operations::OperationSnapshot {
                    proposed_only: false,
                    tool_use_id: "r2".into(),
                    tool: "memory".into(),
                    input_json: json!({"action": "search", "query": "logs"}).to_string(),
                    output: None,
                },
            ],
        )
        .await
        .unwrap();

    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    status_of(&server, "t-read", "done").await;

    let sent = mock.received.lock().unwrap()[0].messages.clone();
    let results: Vec<_> = sent
        .last()
        .unwrap()
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        results.len(),
        2,
        "both cut-off reads get a result: {sent:?}"
    );
    assert!(results
        .iter()
        .all(|(_, text)| text.starts_with("Error: the process stopped")));
    let statuses: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM tool_operations WHERE turn_id='t-read' ORDER BY tool_use_id",
    )
    .fetch_all(&server.pool)
    .await
    .unwrap();
    assert_eq!(
        statuses,
        ["failed", "failed"],
        "no intent is left for a human to reconcile"
    );
}

#[tokio::test]
async fn a_cut_off_batch_with_any_write_still_waits_for_a_human() {
    let mock = Arc::new(Mock::new(vec![Mock::text("must not run")]));
    let server = server(Some(mock.clone())).await;
    let memory = cut_off(&[
        ("r1", "read_file", json!({"path": "/tmp/app.log"})),
        (
            "w1",
            "run_command",
            json!({"command": "systemctl restart app"}),
        ),
    ]);
    server
        .checkpoints
        .save(
            "t-mixed",
            "test",
            "look into the logs",
            &memory,
            1,
            "{}",
            None,
        )
        .await
        .unwrap();
    let (tx, _rx) = mpsc::unbounded_channel();
    resume::reconcile(&server, &EventSink::new(tx)).await;
    status_of(&server, "t-mixed", "suspended").await;
    assert!(mock.received.lock().unwrap().is_empty());
    let kept = server.checkpoints.get("t-mixed").await.unwrap().unwrap();
    assert_eq!(
        kept.messages_json, memory,
        "nothing is settled when a human must decide"
    );
}

#[tokio::test]
async fn a_failed_recovery_admission_emits_no_started_or_resumed_event() {
    let mock = Arc::new(Mock::new(vec![]));
    let (server, _home) = server_with_path(Some(mock.clone())).await;
    server
        .tasks
        .create(jeeves_state::NewTask {
            id: "t",
            session_id: Some("test"),
            title: "resume",
            goal_condition: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    server
        .runs
        .start("r", "t", Some("test"), Some("u"))
        .await
        .unwrap();
    server
        .checkpoints
        .save_round(
            "u",
            "test",
            "resume",
            "[]",
            1,
            "{}",
            Some("r"),
            Some("round"),
        )
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER refuse_reopen BEFORE UPDATE OF status ON task_runs WHEN NEW.status='running' BEGIN SELECT RAISE(ABORT, 'fixture reopen failure'); END")
        .execute(&server.pool).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let row = server.checkpoints.get("u").await.unwrap().unwrap();
    assert!(!resume::resume_turn(&server, &EventSink::new(tx), row, None).await);
    while let Ok(frame) = rx.try_recv() {
        assert!(
            !matches!(
                frame,
                Frame::Notification(
                    ServerNotification::TurnStarted(_) | ServerNotification::TurnResumed(_)
                )
            ),
            "failed admission announced execution"
        );
    }
    assert_eq!(
        server.tasks.get("t").await.unwrap().unwrap().status,
        jeeves_state::TaskStatus::NeedsAttention
    );
    assert!(mock.received.lock().unwrap().is_empty());
}
