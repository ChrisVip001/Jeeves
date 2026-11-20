//! Wait only for jobs belonging to a task already authorized by a human.
use crate::{EventSink, Server};
use jeeves_state::waits::{JobWait, JobWaits, JOB_WAIT_REASON};

pub(crate) fn arm(server: Server, sink: EventSink, wait: JobWait) {
    tokio::spawn(async move {
        let waits = JobWaits::new(server.pool.clone());
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let current = match waits.get(&wait.task_id).await {
                Ok(Some(row))
                    if row.state == "waiting"
                        && row.run_id == wait.run_id
                        && row.turn_id == wait.turn_id =>
                {
                    row
                }
                Ok(_) => return,
                Err(e) => {
                    eprintln!("(jeeves) task wait unavailable: {e}");
                    continue;
                }
            };
            let task = match server.tasks.get(&wait.task_id).await {
                Ok(Some(task)) => task,
                Ok(None) => return,
                Err(e) => {
                    eprintln!("(jeeves) waiting task unavailable: {e}");
                    continue;
                }
            };
            if task.status != jeeves_state::TaskStatus::Suspended
                || task.suspend_reason.as_deref() != Some(JOB_WAIT_REASON)
            {
                if let Err(e) = waits.cancel_obsolete(&wait).await {
                    eprintln!("(jeeves) obsolete task wait could not be cleared: {e}");
                    continue;
                }
                return;
            }
            let jobs = match waits.jobs(&wait.task_id).await {
                Ok(jobs) => jobs,
                Err(e) => {
                    eprintln!("(jeeves) waiting job states unavailable: {e}");
                    continue;
                }
            };
            if jobs.iter().any(|job| job.status == "running") {
                continue;
            }
            let coding_failure = crate::task::worktree::owned(&server.settings().home, Some(&task))
                .is_some()
                && jobs.iter().any(|job| job.status == "failed");
            if coding_failure
                || jobs.is_empty()
                || jobs
                    .iter()
                    .any(|job| matches!(job.status.as_str(), "interrupted" | "killed"))
            {
                let reason = if coding_failure {
                    "The coding executor failed; review the worktree and job log before continuing"
                } else {
                    "A background job was interrupted or stopped; reconcile before continuing"
                };
                let lock = server.session_write_lock(&format!("task-control:{}", wait.task_id));
                let _guard = lock.lock().await;
                if review(&server, &sink, &wait, reason).await {
                    return;
                }
                continue;
            }
            if server.resolved().is_none()
                || !server.active_turns(Some(&wait.session_id)).is_empty()
            {
                continue;
            }
            if crate::resume::resume_waiting(&server, &sink, current, &jobs).await {
                return;
            }
        }
    });
}

/// A damaged or interrupted job wait stops its own attempt durably before notifying the owner.
pub(crate) async fn review(
    server: &Server,
    sink: &EventSink,
    wait: &JobWait,
    reason: &str,
) -> bool {
    if let Err(e) = server
        .checkpoints
        .review(&wait.turn_id, Some(&wait.run_id), reason)
        .await
    {
        eprintln!(
            "(jeeves) could not record review of job wait {}: {e}",
            wait.task_id
        );
        return false;
    }
    eprintln!("(jeeves) job wait {} stopped: {reason}", wait.task_id);
    if let Ok(Some(task)) = server.tasks.get(&wait.task_id).await {
        if task.last_stop_reason.as_deref() == Some(reason) {
            crate::task::notify(sink, &task, Some(reason));
        }
    }
    true
}
