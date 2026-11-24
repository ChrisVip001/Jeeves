use super::*;

#[tokio::test]
async fn restart_requires_attention_and_old_instance_cannot_update_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    jeeves_state::Tasks::new(pool.clone())
        .create(jeeves_state::NewTask {
            id: "browser-test",
            session_id: None,
            title: "fixture",
            goal_condition: None,
            goal_draft: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    let host = BrowserHost::new(pool.clone(), None).await;
    assert!(!host.available());
    let mut first = store::create(
        &pool,
        "browser-test",
        "owner",
        &["https://example.com".into()],
        "https://example.com",
    )
    .await
    .unwrap();
    first.status = "agent".into();
    store::update(&pool, &first, first.generation)
        .await
        .unwrap();
    drop(host);
    let _restarted = BrowserHost::new(pool.clone(), None).await;
    let stopped = store::get(&pool, "browser-test").await.unwrap().unwrap();
    assert_eq!(stopped.status, "needsAttention");
    assert!(stopped.generation > first.generation);
    let second = store::create(
        &pool,
        "browser-test",
        "owner",
        &["https://example.com".into()],
        "https://example.com",
    )
    .await
    .unwrap();
    assert_ne!(first.id, second.id);
    let abandoned_id = second.id.clone();
    let second = store::create(
        &pool,
        "browser-test",
        "owner",
        &["https://example.com".into()],
        "https://example.com",
    )
    .await
    .unwrap();
    assert_ne!(second.id, abandoned_id);

    first.generation = second.generation + 100;
    first.status = "human".into();
    store::update(&pool, &first, first.generation)
        .await
        .unwrap();
    let current = store::get(&pool, "browser-test").await.unwrap().unwrap();
    assert_eq!(current.id, second.id);
    assert_eq!(current.status, "starting");
}

async fn task(pool: &sqlx::SqlitePool, id: &str) {
    jeeves_state::Tasks::new(pool.clone())
        .create(jeeves_state::NewTask {
            id,
            session_id: None,
            title: "fixture",
            goal_condition: None,
            goal_draft: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
}

async fn session_in(pool: &sqlx::SqlitePool, id: &str, status: &str) {
    let mut row = store::create(
        pool,
        id,
        "owner",
        &["https://example.com".into()],
        "https://example.com/inbox?token=secret",
    )
    .await
    .unwrap();
    row.status = status.into();
    store::update(pool, &row, row.generation).await.unwrap();
}

#[tokio::test]
async fn a_takeover_outlives_a_restart_and_says_so_without_leaking_the_url() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    task(&pool, "held").await;
    session_in(&pool, "held", "human").await;
    let _host = BrowserHost::new(pool.clone(), None).await;
    assert!(is_held(&pool, "held").await.unwrap());
    let row = store::get(&pool, "held").await.unwrap().unwrap();
    assert_eq!(row.status, "needsAttention");
    let message = row.error.unwrap();
    assert!(message.contains("in control"), "{message}");
    assert!(
        message.contains("https://example.com"),
        "names the origin: {message}"
    );
    assert!(!message.contains("secret"), "never the query: {message}");
}

#[tokio::test]
async fn the_hold_survives_a_second_restart_and_an_agent_session_is_not_a_hold() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    task(&pool, "held").await;
    task(&pool, "agent").await;
    session_in(&pool, "held", "human").await;
    session_in(&pool, "agent", "agent").await;
    let _first = BrowserHost::new(pool.clone(), None).await;
    let since = store::hold_get(&pool, "held").await.unwrap().unwrap().url;
    let _second = BrowserHost::new(pool.clone(), None).await;
    assert!(
        is_held(&pool, "held").await.unwrap(),
        "still held after another restart"
    );
    assert_eq!(
        store::hold_get(&pool, "held").await.unwrap().unwrap().url,
        since
    );
    assert!(!is_held(&pool, "agent").await.unwrap());
}

#[tokio::test]
async fn a_hold_is_cleared_only_by_the_fresh_observation() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    task(&pool, "held").await;
    session_in(&pool, "held", "human").await;
    let _host = BrowserHost::new(pool.clone(), None).await;
    store::create(
        &pool,
        "held",
        "owner",
        &["https://example.com".into()],
        "https://example.com",
    )
    .await
    .unwrap();
    assert!(is_held(&pool, "held").await.unwrap());
    let mut fresh = store::get(&pool, "held").await.unwrap().unwrap();
    assert!(
        store::hold_clear(&pool, "held", &fresh.id, fresh.generation)
            .await
            .is_err()
    );
    fresh.status = "agent".into();
    fresh.observed_at = Some("2026-01-01T00:00:00Z".into());
    store::update(&pool, &fresh, fresh.generation)
        .await
        .unwrap();
    store::hold_clear(&pool, "held", &fresh.id, fresh.generation)
        .await
        .unwrap();
    assert!(!is_held(&pool, "held").await.unwrap());
}

#[tokio::test]
async fn emergency_stop_fences_every_browser_until_manual_release() {
    use crate::input_wake_tests::{fixture, rpc};
    use serde_json::json;
    let (_home, server, _) = fixture().await;
    for id in ["phone-task", "mac-task"] {
        task(&server.pool, id).await;
        session_in(&server.pool, id, "agent").await;
    }
    let stopped = rpc(&server, "executor/stop", json!({})).await.unwrap();
    assert_eq!(stopped["stopped"], true);
    assert_eq!(
        rpc(&server, "executor/stop", json!({})).await.unwrap(),
        stopped
    );
    for id in ["phone-task", "mac-task"] {
        let row = store::get(&server.pool, id).await.unwrap().unwrap();
        assert_eq!(row.status, "needsAttention");
        assert!(store::is_held(&server.pool, id).await.unwrap());
    }
    assert!(
        rpc(&server, "executor/release", json!({"expectedGeneration":0}))
            .await
            .is_err()
    );
    let released = rpc(
        &server,
        "executor/release",
        json!({"expectedGeneration":stopped["generation"]}),
    )
    .await
    .unwrap();
    assert_eq!(released["stopped"], false);
    assert!(store::is_held(&server.pool, "phone-task").await.unwrap());
}

#[tokio::test]
async fn a_late_worker_receipt_cannot_restore_authority_on_takeover_or_stop() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    let host = BrowserHost::new(pool.clone(), None).await;
    task(&pool, "late").await;
    session_in(&pool, "late", "agent").await;
    let old = store::get(&pool, "late").await.unwrap().unwrap();
    let mut human = old.clone();
    human.generation += 1;
    human.status = "human".into();
    assert!(store::update(&pool, &human, old.generation).await.unwrap());
    let delayed = BrowserWorkerState {
        generation: old.generation,
        status: "paused".into(),
        observation: None,
        blocked_origins: vec![],
    };
    assert!(
        !host
            .record_reply(old.clone(), old.generation, &delayed)
            .await
            .unwrap()
    );
    assert_eq!(
        store::get(&pool, "late").await.unwrap().unwrap().status,
        "human"
    );
    let future = BrowserWorkerState {
        generation: human.generation + 10,
        ..delayed.clone()
    };
    assert!(
        host.record_reply(human.clone(), human.generation, &future)
            .await
            .is_err()
    );
    let stopped = host.emergency_stop().await.unwrap();
    let late_human = BrowserWorkerState {
        generation: human.generation,
        ..delayed
    };
    assert!(
        !host
            .record_reply(human.clone(), human.generation, &late_human)
            .await
            .unwrap()
    );
    let restarted = BrowserHost::new(pool.clone(), None).await;
    assert_eq!(
        restarted.executor_status().await.unwrap().generation,
        stopped.generation
    );
    assert_eq!(
        store::get(&pool, "late").await.unwrap().unwrap().status,
        "needsAttention"
    );
}

#[tokio::test]
async fn a_failed_stop_transaction_blocks_launch_and_release_until_retried() {
    use crate::input_wake_tests::{fixture, rpc};
    use serde_json::json;
    let (_home, server, _) = fixture().await;
    task(&server.pool, "storage-error").await;
    session_in(&server.pool, "storage-error", "agent").await;
    sqlx::query("CREATE TRIGGER reject_hold BEFORE INSERT ON browser_holds BEGIN SELECT RAISE(ABORT,'fixture write failure'); END")
        .execute(&server.pool).await.unwrap();
    assert!(
        rpc(&server, "executor/stop", json!({}))
            .await
            .unwrap_err()
            .contains("fixture write failure")
    );
    assert!(rpc(&server, "executor/control", json!({})).await.is_err());
    assert!(
        rpc(&server, "executor/release", json!({"expectedGeneration":0}))
            .await
            .is_err()
    );
    assert!(
        rpc(
            &server,
            "browser/start",
            json!({"taskId":"storage-error","url":"https://example.com"})
        )
        .await
        .unwrap_err()
        .contains("Executors are stopped")
    );
    sqlx::query("DROP TRIGGER reject_hold")
        .execute(&server.pool)
        .await
        .unwrap();
    let stopped = rpc(&server, "executor/stop", json!({})).await.unwrap();
    assert_eq!(stopped["generation"], 1);
    assert!(store::is_held(&server.pool, "storage-error").await.unwrap());
}

#[tokio::test]
async fn remote_browser_input_requires_a_live_task_browser() {
    let (_home, server, _) = crate::input_wake_tests::fixture().await;
    let error = crate::input_wake_tests::rpc(&server,"browser/input",serde_json::json!({
        "taskId":"missing","generation":1,"frameId":"expired","input":{"kind":"click","x":10,"y":10}
    })).await.unwrap_err();
    assert!(error.contains("open this task's browser"), "{error}");
}

#[tokio::test]
async fn accepted_download_recovers_when_artifact_recording_was_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    task(&pool, "reports").await;
    let id = "f6bbb290-7300-44ea-9e60-532bc26e1661";
    sqlx::query("INSERT INTO browser_downloads VALUES('browser',?,'reports','../same.txt',?)")
        .bind(id)
        .bind(b"approved".as_slice())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fixture_offline BEFORE INSERT ON task_artifacts BEGIN SELECT RAISE(ABORT,'fixture recording failure'); END").execute(&pool).await.unwrap();
    downloads::recover(&pool, dir.path()).await.unwrap();
    let path = dir
        .path()
        .join("artifacts/reports")
        .join(format!("{id}-.._same.txt"));
    assert_eq!(std::fs::read(&path).unwrap(), b"approved");
    let artifacts = jeeves_state::Artifacts::new(pool.clone());
    assert!(artifacts.list("reports", 10).await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER fixture_offline")
        .execute(&pool)
        .await
        .unwrap();
    std::fs::write(&path, b"local change").unwrap();
    downloads::recover(&pool, dir.path()).await.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"local change");
    assert!(artifacts.list("reports", 10).await.unwrap().is_empty());
    std::fs::write(&path, b"approved").unwrap();
    for _ in 0..2 {
        downloads::recover(&pool, dir.path()).await.unwrap();
    }
    let rows = artifacts.list("reports", 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, path.to_str().unwrap());
    let stored: bool =
        sqlx::query_scalar("SELECT body IS NULL FROM browser_downloads WHERE download_id=?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(stored);
}
