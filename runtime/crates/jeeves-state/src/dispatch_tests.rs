use crate::{NewTask, TaskStatus, Tasks, dispatch::TaskDispatch};

async fn fixture() -> (tempfile::TempDir, sqlx::SqlitePool, Tasks, TaskDispatch) {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
    (
        dir,
        pool.clone(),
        Tasks::new(pool.clone()),
        TaskDispatch::new(pool),
    )
}
async fn add(tasks: &Tasks, id: &str) {
    tasks
        .create(NewTask {
            id,
            session_id: Some("chat"),
            title: id,
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

#[tokio::test]
async fn dependent_dispatch_waits_for_each_verified_completion_after_restart() {
    let (dir, pool, tasks, queue) = fixture().await;
    for id in [
        "source-a",
        "source-b",
        "dependent",
        "paused",
        "cancelled",
        "independent",
    ] {
        add(&tasks, id).await;
    }
    let mut conn = pool.acquire().await.unwrap();
    for id in ["dependent", "paused", "cancelled"] {
        for prerequisite in ["source-a", "source-b"] {
            TaskDispatch::add_dependency_on(&mut conn, id, prerequisite)
                .await
                .unwrap();
        }
        queue.enqueue(id, id).await.unwrap();
    }
    drop(conn);
    queue
        .enqueue("independent", "standalone work")
        .await
        .unwrap();
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "independent");
    queue.finish("independent").await.unwrap();
    tasks.finish("source-a", TaskStatus::Done).await.unwrap();
    assert!(queue.claim_leased("worker", 30, 2).await.unwrap().is_none());
    assert_eq!(
        queue.unresolved_dependencies("dependent").await.unwrap(),
        ["source-a", "source-b"]
    );
    tasks.suspend("paused", Some("human pause")).await.unwrap();
    tasks
        .finish("cancelled", TaskStatus::Cancelled)
        .await
        .unwrap();
    drop((tasks, queue));
    pool.close().await;

    let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    let queue = TaskDispatch::new(pool.clone());
    assert!(queue.claim_leased("worker", 30, 2).await.unwrap().is_none());
    measure_prerequisite(&pool, "source-a", "a-verified", true).await;
    measure_prerequisite(&pool, "source-b", "b-failed", false).await;
    assert_eq!(
        queue.unresolved_dependencies("dependent").await.unwrap(),
        ["source-b"]
    );
    assert!(!queue.has_ready_dependents("source-b").await.unwrap());
    measure_prerequisite(&pool, "source-b", "b-verified", true).await;
    assert!(queue.has_ready_dependents("source-b").await.unwrap());
    let (left, right) = tokio::join!(
        queue.claim_leased("first-worker", 30, 2),
        queue.claim_leased("second-worker", 30, 2)
    );
    let claims: Vec<_> = [left.unwrap(), right.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].task_id, "dependent");
    queue.finish("dependent").await.unwrap();
    assert!(queue.resume_unstarted("paused").await.unwrap());
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "paused");
    queue.finish("paused").await.unwrap();
    assert!(queue.claim().await.unwrap().is_none());
    assert_eq!(
        tasks.get("cancelled").await.unwrap().unwrap().status,
        TaskStatus::Cancelled
    );
}

#[tokio::test]
async fn a_new_dependency_on_a_reopened_task_requires_fresh_evidence() {
    let (_dir, pool, tasks, queue) = fixture().await;
    for id in ["source", "before", "after"] {
        add(&tasks, id).await;
    }
    measure_prerequisite(&pool, "source", "initial", true).await;
    let mut conn = pool.acquire().await.unwrap();
    TaskDispatch::add_dependency_on(&mut conn, "before", "source")
        .await
        .unwrap();
    tasks.set_running("source", "revision").await.unwrap();
    TaskDispatch::add_dependency_on(&mut conn, "after", "source")
        .await
        .unwrap();
    assert!(
        queue
            .unresolved_dependencies("before")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        queue.unresolved_dependencies("after").await.unwrap(),
        ["source"]
    );
    measure_prerequisite(&pool, "source", "revised", true).await;
    assert!(
        queue
            .unresolved_dependencies("after")
            .await
            .unwrap()
            .is_empty()
    );
}

async fn measure_prerequisite(pool: &sqlx::SqlitePool, id: &str, run: &str, passed: bool) {
    let acceptance = r#"[{"kind":"toolsOk"}]"#;
    sqlx::query("UPDATE agent_tasks SET acceptance_json=? WHERE id=?")
        .bind(acceptance)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    let tasks = Tasks::new(pool.clone());
    crate::Runs::new(pool.clone())
        .start(run, id, Some("work"), Some(run))
        .await
        .unwrap();
    tasks.set_running(id, run).await.unwrap();
    let checks = serde_json::json!([{"label":"checks", "passed":passed}]).to_string();
    crate::verification::Verifications::new(pool.clone())
        .record(id, run, run, acceptance, &checks, &[])
        .await
        .unwrap();
    if passed {
        tasks
            .finish_run(id, TaskStatus::Done, None, Some(run))
            .await
            .unwrap();
    } else {
        assert!(
            tasks
                .finish_run(id, TaskStatus::Done, None, Some(run))
                .await
                .is_err()
        );
        tasks
            .needs_attention(id, "failed verification")
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn dispatch_claim_is_single_and_keeps_conversation_separate() {
    let (_dir, _pool, tasks, queue) = fixture().await;
    add(&tasks, "one").await;
    add(&tasks, "two").await;
    queue.enqueue("one", "first").await.unwrap();
    queue.enqueue("two", "second").await.unwrap();
    assert!(tasks.active("chat").await.unwrap().is_none());
    assert_eq!(tasks.latest("chat").await.unwrap().unwrap().id, "two");
    let (a, b) = tokio::join!(queue.claim(), queue.claim());
    let claimed = [a.unwrap(), b.unwrap()];
    assert_eq!(claimed.iter().flatten().count(), 1);
    let first = claimed.into_iter().flatten().next().unwrap();
    assert_eq!(first.task_id, "one");
    assert_ne!(first.session_id, "chat");
    assert!(queue.claim().await.unwrap().is_none());
    queue.finish("one").await.unwrap();
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "two");
    assert!(queue.enqueue("one", "do not repeat").await.is_err());
}
#[tokio::test]
async fn interrupted_claim_without_checkpoint_is_not_replayed() {
    let (_dir, _pool, tasks, queue) = fixture().await;
    add(&tasks, "one").await;
    queue.enqueue("one", "first").await.unwrap();
    queue.claim().await.unwrap().unwrap();
    assert_eq!(queue.reconcile().await.unwrap(), vec!["one"]);
    assert_eq!(
        tasks.get("one").await.unwrap().unwrap().status,
        TaskStatus::NeedsAttention
    );
    assert!(queue.claim().await.unwrap().is_none());
    assert!(queue.reconcile().await.unwrap().is_empty());
    assert!(queue.resume_unstarted("one").await.unwrap());
    assert_eq!(
        tasks.get("one").await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "one");
}
#[tokio::test]
async fn existing_checkpoint_owns_recovery_and_pending_dispatch_survives_restart() {
    let (_dir, pool, tasks, queue) = fixture().await;
    add(&tasks, "one").await;
    add(&tasks, "two").await;
    let row = queue.enqueue("one", "first").await.unwrap();
    queue.enqueue("two", "second").await.unwrap();
    queue.claim().await.unwrap().unwrap();
    crate::Checkpoints::new(pool.clone())
        .save(&row.turn_id, &row.session_id, "first", "[]", 0, "{}", None)
        .await
        .unwrap();
    assert!(queue.reconcile().await.unwrap().is_empty());
    assert_eq!(
        tasks.get("one").await.unwrap().unwrap().status,
        TaskStatus::Pending
    );
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "two");
}
#[tokio::test]
async fn paused_and_cancelled_dispatches_do_not_run() {
    let (_dir, pool, tasks, queue) = fixture().await;
    for id in ["paused", "cancelled"] {
        add(&tasks, id).await;
        queue.enqueue(id, id).await.unwrap();
    }
    sqlx::query("UPDATE agent_tasks SET status='suspended' WHERE id='paused'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE agent_tasks SET status='cancelled' WHERE id='cancelled'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(queue.claim().await.unwrap().is_none());
    sqlx::query("UPDATE agent_tasks SET status='pending' WHERE id='paused'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(queue.claim().await.unwrap().unwrap().task_id, "paused");
}

#[tokio::test]
async fn progress_is_owner_scoped_and_does_not_claim_unverified_completion() {
    let (_dir, pool, tasks, queue) = fixture().await;
    add(&tasks, "one").await;
    queue.enqueue("one", "work").await.unwrap();
    sqlx::query("UPDATE agent_tasks SET status='done' WHERE id='one'")
        .execute(&pool)
        .await
        .unwrap();
    let rows = queue.progress("chat").await.unwrap();
    assert_eq!(rows[0]["status"], "produced_unverified");
    assert!(queue.progress("another-chat").await.unwrap().is_empty());
}

#[tokio::test]
async fn leased_claims_run_side_by_side_up_to_the_worker_count() {
    let (_dir, _pool, tasks, queue) = fixture().await;
    for id in ["one", "two", "three"] {
        add(&tasks, id).await;
        queue.enqueue(id, "work").await.unwrap();
    }
    let a = queue.claim_leased("w0", 60, 2).await.unwrap().unwrap();
    let b = queue.claim_leased("w1", 60, 2).await.unwrap().unwrap();
    assert_ne!(a.task_id, b.task_id);
    assert!(
        queue.claim_leased("w2", 60, 2).await.unwrap().is_none(),
        "a third claim waits for a free slot"
    );
    queue.finish(&a.task_id).await.unwrap();
    assert_eq!(
        queue
            .claim_leased("w2", 60, 2)
            .await
            .unwrap()
            .unwrap()
            .task_id,
        "three"
    );
}

#[tokio::test]
async fn a_lease_that_is_not_renewed_is_swept_once() {
    let (_dir, _pool, tasks, queue) = fixture().await;
    add(&tasks, "one").await;
    queue.enqueue("one", "work").await.unwrap();
    queue.claim_leased("w0", -1, 2).await.unwrap().unwrap();
    let expired = queue.expire_stale().await.unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].task_id, "one");
    assert_eq!(expired[0].owner, "w0");
    assert!(
        queue.expire_stale().await.unwrap().is_empty(),
        "the second sweep finds nothing: the task resumes once"
    );
    assert!(
        queue.claim_leased("w1", 60, 2).await.unwrap().is_none(),
        "an interrupted dispatch is never claimed again"
    );
}

#[tokio::test]
async fn a_renewed_or_finished_lease_is_never_swept() {
    let (_dir, _pool, tasks, queue) = fixture().await;
    for id in ["one", "two"] {
        add(&tasks, id).await;
        queue.enqueue(id, "work").await.unwrap();
    }
    let a = queue.claim_leased("w0", -1, 2).await.unwrap().unwrap();
    let b = queue.claim_leased("w1", -1, 2).await.unwrap().unwrap();
    assert!(queue.heartbeat(&a.task_id, "w0", 60).await.unwrap());
    queue.finish(&b.task_id).await.unwrap();
    assert!(queue.expire_stale().await.unwrap().is_empty());
    assert!(
        !queue
            .heartbeat(&a.task_id, "someone-else", 60)
            .await
            .unwrap(),
        "only the owner renews"
    );
}
