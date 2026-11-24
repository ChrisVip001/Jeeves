use crate::{NewTask, Tasks};

/// A drafted "done when" line is inert storage: it never becomes the judged condition on its own,
/// and once answered it is gone, so the person is never asked about the same draft twice.
#[tokio::test]
async fn a_goal_draft_only_becomes_the_condition_through_a_confirmation() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    let new = |id| NewTask {
        id,
        session_id: Some("chat"),
        title: "work",
        goal_condition: None,
        goal_draft: Some("every duty names its team"),
        acceptance_json: "[]",
        budget_json: "{}",
        cwd: None,
        max_rounds: 3,
        tier: None,
    };
    tasks.create(new("accepted")).await.unwrap();
    tasks.create(new("declined")).await.unwrap();

    let stored = tasks.get("accepted").await.unwrap().unwrap();
    assert_eq!(
        stored.goal_draft.as_deref(),
        Some("every duty names its team")
    );
    assert_eq!(stored.goal_condition, None, "the draft judges nothing yet");

    assert!(tasks.confirm_goal_draft("accepted", true).await.unwrap());
    let row = tasks.get("accepted").await.unwrap().unwrap();
    assert_eq!(
        row.goal_condition.as_deref(),
        Some("every duty names its team")
    );
    assert_eq!(row.goal_draft, None);
    assert!(
        !tasks.confirm_goal_draft("accepted", true).await.unwrap(),
        "an answered draft cannot be answered again"
    );

    assert!(tasks.confirm_goal_draft("declined", false).await.unwrap());
    let row = tasks.get("declined").await.unwrap().unwrap();
    assert_eq!(row.goal_draft, None);
    assert_eq!(row.goal_condition, None, "declining judges nothing");
}

/// A task that already carries a human-set goal keeps it: a draft cannot overwrite it.
#[tokio::test]
async fn a_confirmed_goal_is_never_replaced_by_a_draft() {
    let home = tempfile::tempdir().unwrap();
    let pool = crate::open(&home.path().join("state.db")).await.unwrap();
    let tasks = Tasks::new(pool.clone());
    tasks
        .create(NewTask {
            id: "task",
            session_id: Some("chat"),
            title: "work",
            goal_condition: Some("the human's own condition"),
            goal_draft: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 3,
            tier: None,
        })
        .await
        .unwrap();
    sqlx::query("UPDATE agent_tasks SET goal_draft=? WHERE id=?")
        .bind("a drafted replacement")
        .bind("task")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        !tasks.confirm_goal_draft("task", true).await.unwrap(),
        "accepting cannot overwrite a condition the person already set"
    );
    let row = tasks.get("task").await.unwrap().unwrap();
    assert_eq!(
        row.goal_condition.as_deref(),
        Some("the human's own condition")
    );
}
