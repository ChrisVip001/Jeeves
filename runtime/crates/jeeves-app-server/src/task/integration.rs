//! A person links durable coding tasks and admits a specific child checkpoint.
//! Git changes only the pending parent's worktree. The parent still runs its own final checks.
use super::git_checkpoint::{git, validate_root};
use super::{invalid, reload, worktree};
use crate::{Server, dispatch::internal};
use jeeves_agent::verify::{Acceptance, parse_checked};
use jeeves_protocol::{ErrorObject, TaskGitCheckpointResponse, TaskIntegrateParams};
use jeeves_state::TaskStatus;
use std::path::Path;

pub(super) async fn parent_root(server: &Server, id: &str) -> Result<String, ErrorObject> {
    let task = reload(server, id).await?;
    let queued = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .get(id)
        .await
        .map_err(internal)?
        .is_some();
    if task.status != TaskStatus::Pending || task.current_turn_id.is_some() || queued {
        return Err(invalid("the parent must be pending and not yet queued"));
    }
    if !parse_checked(&task.acceptance_json)
        .map_err(invalid)?
        .iter()
        .any(|check| matches!(check, Acceptance::CommandSucceeds { .. }))
    {
        return Err(invalid(
            "declare a final command check before attaching coding children",
        ));
    }
    let (root, branch) = worktree::of(&server.settings().home, id, task.cwd.as_deref())
        .ok_or_else(|| invalid("a coding parent needs a managed worktree"))?;
    let path = root.clone();
    tokio::task::spawn_blocking(move || validate_root(Path::new(&path), &branch))
        .await
        .map_err(internal)?
        .map_err(invalid)?;
    Ok(root)
}

pub(crate) async fn require_integrated(server: &Server, id: &str) -> Result<(), ErrorObject> {
    let next: Option<String> = sqlx::query_scalar(
        "SELECT id FROM agent_tasks WHERE parent_task_id=? AND integrated_commit IS NULL LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&server.pool)
    .await
    .map_err(internal)?;
    if let Some(child) = next {
        return Err(invalid(format!(
            "child {child} still needs integration before parent verification"
        )));
    }
    Ok(())
}

fn merge(
    parent: &Path,
    child: &Path,
    child_branch: &str,
    expected: &str,
) -> Result<String, String> {
    validate_root(child, child_branch)?;
    if git(child, &["rev-parse", "HEAD"])? != expected {
        return Err("the reviewed commit no longer matches the child head".into());
    }
    let base = git(parent, &["merge-base", "HEAD", expected])?;
    if base == expected {
        return git(parent, &["rev-parse", "HEAD"]);
    }
    if !git(parent, &["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return Err("review the parent's staged or modified files before integration".into());
    }
    git(parent, &["merge-tree", "--write-tree", "HEAD", expected])
        .map_err(|_| "Git could not merge the child cleanly; resolve it in the parent worktree and confirm again".to_owned())?;
    git(
        parent,
        &[
            "merge",
            "--no-edit",
            "--no-overwrite-ignore",
            "--no-gpg-sign",
            expected,
        ],
    )?;
    git(parent, &["rev-parse", "HEAD"])
}

pub(crate) async fn integrate(
    server: &Server,
    p: TaskIntegrateParams,
) -> Result<TaskGitCheckpointResponse, ErrorObject> {
    if !p.confirmed || p.parent_task_id == p.child_task_id {
        return Err(invalid("confirm the exact commit of a distinct child task"));
    }
    let mut ids = [&p.parent_task_id, &p.child_task_id];
    ids.sort();
    let first = server.session_write_lock(&format!("task-control:{}", ids[0]));
    let second = server.session_write_lock(&format!("task-control:{}", ids[1]));
    let (_first, _second) = (first.lock().await, second.lock().await);
    let child = reload(server, &p.child_task_id).await?;
    if child.parent_task_id.as_deref() != Some(&p.parent_task_id)
        || child.status != TaskStatus::Done
    {
        return Err(invalid(
            "integration requires a completed child belonging to this parent",
        ));
    }
    let root = parent_root(server, &p.parent_task_id).await?;
    let (child_root, branch) =
        worktree::of(&server.settings().home, &child.id, child.cwd.as_deref())
            .ok_or_else(|| invalid("child worktree unavailable"))?;
    let expected = p.commit.clone();
    let merged = tokio::task::spawn_blocking(move || {
        merge(Path::new(&root), Path::new(&child_root), &branch, &expected)
    })
    .await
    .map_err(internal)?
    .map_err(invalid)?;
    let mut tx = server.pool.begin().await.map_err(internal)?;
    let changed = sqlx::query(
        "UPDATE agent_tasks SET integrated_commit=? WHERE id=? AND integrated_commit IS NOT ?",
    )
    .bind(&p.commit)
    .bind(&child.id)
    .bind(&p.commit)
    .execute(&mut *tx)
    .await
    .map_err(internal)?
    .rows_affected();
    if changed != 0 {
        let detail =
            serde_json::json!({"childId":child.id,"childCommit":p.commit,"parentCommit":merged})
                .to_string();
        sqlx::query("INSERT INTO task_events(task_id,seq,kind,detail) SELECT ?,COALESCE(MAX(seq),0)+1,'child_integrated',? FROM task_events WHERE task_id=?")
            .bind(&p.parent_task_id).bind(detail).bind(&p.parent_task_id).execute(&mut *tx).await.map_err(internal)?;
    }
    tx.commit().await.map_err(internal)?;
    Ok(TaskGitCheckpointResponse {
        branch: worktree::branch_of(&p.parent_task_id),
        task_id: p.parent_task_id,
        commit: merged,
    })
}
