use crate::{completions::Completions, NewTask, TaskStatus, Tasks};

#[tokio::test]
async fn a_failed_notice_insert_rolls_back_the_task_transition() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    tasks
        .create(NewTask {
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
    let completions = Completions::new(pool.clone());
    completions
        .bind("task", true, Some("original"))
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_notice BEFORE INSERT ON task_completions BEGIN SELECT RAISE(FAIL,'injected completion failure'); END").execute(&pool).await.unwrap();
    assert!(tasks.finish("task", TaskStatus::Done).await.is_err());
    assert_eq!(
        tasks.get("task").await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
    assert!(completions.pending().await.unwrap().is_empty());
    sqlx::query("DROP TRIGGER fail_notice")
        .execute(&pool)
        .await
        .unwrap();
    tasks.finish("task", TaskStatus::Done).await.unwrap();
    tasks.finish("task", TaskStatus::Done).await.unwrap();
    let rows = completions.pending().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "produced_unverified");
    assert_eq!(rows[0].notify_target.as_deref(), Some("original"));
}

#[tokio::test]
async fn task_and_exact_run_completion_commit_or_rollback_together() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    tasks
        .create(NewTask {
            id: "task",
            session_id: Some("jeeves-main"),
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
    let runs = crate::Runs::new(pool.clone());
    runs.start("run", "task", Some("chat"), Some("turn"))
        .await
        .unwrap();
    tasks.set_running("task", "turn").await.unwrap();
    sqlx::query("CREATE TRIGGER fail_completion BEFORE INSERT ON task_completions BEGIN SELECT RAISE(FAIL,'injected completion failure'); END").execute(&pool).await.unwrap();
    assert!(tasks
        .finish_run("task", TaskStatus::Done, None, Some("run"))
        .await
        .is_err());
    assert_eq!(
        tasks.get("task").await.unwrap().unwrap().status,
        TaskStatus::Running
    );
    assert_eq!(runs.get("run").await.unwrap().unwrap().status, "running");
    sqlx::query("DROP TRIGGER fail_completion")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_summary BEFORE UPDATE OF content ON chat_log BEGIN SELECT RAISE(FAIL,'injected summary write failure'); END")
        .execute(&pool).await.unwrap();
    assert!(tasks
        .finish_run_with_summary(
            "task",
            TaskStatus::Done,
            None,
            Some("run"),
            Some("Fixture result")
        )
        .await
        .is_err());
    assert_eq!(runs.get("run").await.unwrap().unwrap().status, "running");
    sqlx::query("DROP TRIGGER fail_summary")
        .execute(&pool)
        .await
        .unwrap();
    tasks
        .finish_run_with_summary(
            "task",
            TaskStatus::Done,
            None,
            Some("run"),
            Some("Fixture result"),
        )
        .await
        .unwrap();
    assert_eq!(
        tasks.get("task").await.unwrap().unwrap().status,
        TaskStatus::Done
    );
    assert_eq!(runs.get("run").await.unwrap().unwrap().status, "completed");
    let receipt_run: Option<String> = sqlx::query_scalar("SELECT run_id FROM task_completions WHERE task_id='task' AND status='produced_unverified' ORDER BY id DESC LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(receipt_run.as_deref(), Some("run"));
    tasks
        .finish_run_with_summary(
            "task",
            TaskStatus::Done,
            None,
            Some("run"),
            Some("Repeated result"),
        )
        .await
        .unwrap();
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT content FROM chat_log WHERE source='task_completion'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].ends_with("Fixture result"));
    let persisted: String =
        sqlx::query_scalar("SELECT summary FROM task_completions WHERE run_id='run'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted, "Fixture result");
    let history = crate::Chat::new(pool.clone())
        .messages("jeeves-main", None, 10)
        .await
        .unwrap();
    assert_eq!(history[0].task_id.as_deref(), Some("task"));
    assert_eq!(
        history[0].receipt_status.as_deref(),
        Some("produced_unverified")
    );
    let reason: String =
        sqlx::query_scalar("SELECT reason FROM task_completions WHERE run_id='run'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!reason.contains("Fixture result"));
}

#[tokio::test]
async fn a_model_judged_goal_without_acceptance_is_produced_unverified() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    tasks
        .create(NewTask {
            id: "goal",
            session_id: Some("chat"),
            title: "research",
            goal_condition: Some("Prepare a useful research summary"),
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 3,
            tier: None,
        })
        .await
        .unwrap();
    Completions::new(pool.clone())
        .bind("goal", true, Some("test-target"))
        .await
        .unwrap();
    tasks.finish("goal", TaskStatus::Done).await.unwrap();
    let receipt = Completions::new(pool).pending().await.unwrap().remove(0);
    assert!(receipt.is_goal);
    assert_eq!(receipt.status, "produced_unverified");
}

#[tokio::test]
async fn edits_replace_the_written_version_even_if_the_file_size_is_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let files = crate::Artifacts::new(pool);
    files
        .add_with_text("report", None, "weekly.md", 3, Some("one"))
        .await
        .unwrap();
    files
        .add_with_text("report", None, "weekly.md", 3, Some("two"))
        .await
        .unwrap();
    let first = files.list("report", 10).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].written_text.as_deref(), Some("two"));
    files.add("report", None, "weekly.md", 3).await.unwrap();
    assert!(files.list("report", 10).await.unwrap()[0]
        .written_text
        .is_none());
}
