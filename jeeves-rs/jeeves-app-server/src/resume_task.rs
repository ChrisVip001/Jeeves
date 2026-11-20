//! Task checkpoints are not chat checkpoints: an overnight task with a budget is meant to run
//! for hours, so the chat age limit (`JEEVES_RESUME_MAX_AGE`) does not apply to it.
use crate::Server;
use jeeves_state::{CheckpointRow, TaskStatus};

/// `true` when this checkpoint belongs to a still-open task with a budget that is not spent.
/// A task paused or stopped by a human is refused elsewhere (the pause fence); here we only
/// decide whether *age* alone may abandon it.
pub(crate) async fn exempt_from_age(server: &Server, row: &CheckpointRow) -> bool {
    let Some(run_id) = &row.run_id else {
        return false;
    };
    let Ok(Some(run)) = server.runs.get(run_id).await else {
        return false;
    };
    let Ok(Some(task)) = server.tasks.get(&run.task_id).await else {
        return false;
    };
    if !task.status.is_open() || task.status == TaskStatus::NeedsAttention {
        return false;
    }
    matches!(crate::turn::budget_remaining(&task), Ok(true))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture(budget: &str) -> (Server, CheckpointRow) {
        let dir = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&dir.path().join("state.db"))
            .await
            .unwrap();
        let server = Server::boot(
            pool,
            jeeves_config::Settings {
                home: dir.path().into(),
                ..Default::default()
            },
        )
        .await;
        let _ = dir.keep();
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: "t1",
                session_id: None,
                title: "overnight",
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: budget,
                cwd: None,
                max_rounds: 5,
                tier: None,
            })
            .await
            .unwrap();
        server
            .runs
            .start("turn1#1", "t1", None, Some("turn1"))
            .await
            .unwrap();
        server
            .checkpoints
            .save("turn1", "s", "go", "[]", 1, "{}", Some("turn1#1"))
            .await
            .unwrap();
        let row = server.checkpoints.get("turn1").await.unwrap().unwrap();
        (server, row)
    }

    #[tokio::test]
    async fn a_budgeted_open_task_is_exempt() {
        let (server, row) = fixture(r#"{"tokens":100000}"#).await;
        assert!(exempt_from_age(&server, &row).await);
    }

    #[tokio::test]
    async fn no_budget_no_run_or_a_spent_budget_is_not_exempt() {
        let (server, row) = fixture("{}").await;
        assert!(!exempt_from_age(&server, &row).await, "no budget");
        let (server, mut row) = fixture(r#"{"tokens":100000}"#).await;
        row.run_id = None;
        assert!(!exempt_from_age(&server, &row).await, "a chat checkpoint");
        let (server, row) = fixture(r#"{"tokens":100000}"#).await;
        server
            .tasks
            .finish("t1", TaskStatus::Cancelled)
            .await
            .unwrap();
        assert!(!exempt_from_age(&server, &row).await, "cancelled");
    }
}
