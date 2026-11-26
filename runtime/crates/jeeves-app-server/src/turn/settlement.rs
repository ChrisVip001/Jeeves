use super::{FinishedTurn, TaskDecision};
use crate::{EventSink, Server, dispatch::internal};
use jeeves_protocol::{ErrorObject, ServerNotification, TurnSuspendedNotification, codes};
use jeeves_state::{RunStatus, TaskRow, TaskStatus};

pub(super) struct Settlement {
    pub carried_task: Option<TaskRow>,
    pub waiting_for_jobs: bool,
    pub input_wait_committed: bool,
}

pub(super) async fn settle_task(
    server: &Server,
    sink: &EventSink,
    t: &mut FinishedTurn<'_>,
) -> Result<Settlement, ErrorObject> {
    let decision = t.decision.take();
    let FinishedTurn {
        session_id,
        turn_id,
        result,
        run_id,
        goal_status,
        goal_rounds,
        ..
    } = &*t;
    let paused = server.take_suspending(turn_id);
    let wants_job_wait =
        !paused && !result.interrupted && matches!(decision, Some(TaskDecision::WaitingForJobs(_)));
    let wants_input_wait = !paused
        && !result.interrupted
        && matches!(decision, Some(TaskDecision::WaitingForInput(_)));
    let mut input_wait_committed = false;
    let mut waiting_for_jobs = false;
    let (final_status, suspended_reason) = if paused {
        (
            "suspended",
            Some("Paused; it can resume from the checkpoint"),
        )
    } else if result.interrupted {
        ("abandoned", Some("interrupted by user"))
    } else if result.stop_reason == jeeves_provider::StopReason::UncertainEffect {
        (
            "suspended",
            Some("Tool outcome unknown; reconcile before continuing"),
        )
    } else if wants_job_wait {
        ("waiting_jobs", Some(jeeves_state::waits::JOB_WAIT_REASON))
    } else if wants_input_wait {
        (
            "waiting_input",
            Some(jeeves_state::external_waits::INPUT_WAIT_REASON),
        )
    } else {
        ("done", None)
    };
    if !wants_job_wait
        && !wants_input_wait
        && let Err(e) = server.checkpoints.finish(turn_id, final_status).await
    {
        eprintln!("(jeeves) checkpoint close-out not recorded: {e}");
    }

    let carried_task = server.tasks.by_turn(turn_id).await.map_err(internal)?;
    if let Some(task) = &carried_task {
        let control = server.session_write_lock(&format!("task-control:{}", task.id));
        let _control = control.lock().await;
        let task = server
            .tasks
            .get(&task.id)
            .await
            .map_err(internal)?
            .ok_or_else(|| internal("Task disappeared during settlement"))?;
        let human_paused = jeeves_state::human_pauses::HumanPauses::new(server.pool.clone())
            .is_paused(&task.id)
            .await
            .map_err(|error| ErrorObject {
                code: codes::INTERNAL_ERROR,
                message: format!("Cannot verify human task control: {error}"),
                data: None,
            })?;
        let paused = paused
            || human_paused
            || server.take_suspending(turn_id)
            || (task.status == TaskStatus::Suspended
                && task.suspend_reason.as_deref() != Some(jeeves_state::waits::JOB_WAIT_REASON));
        let (verdict_reason, run_status) = if task.status == TaskStatus::Cancelled {
            server
                .checkpoints
                .finish(turn_id, "abandoned")
                .await
                .map_err(internal)?;
            (
                Some("Cancelled by the user".to_string()),
                RunStatus::Cancelled,
            )
        } else if paused {
            server
                .checkpoints
                .finish(turn_id, "suspended")
                .await
                .map_err(internal)?;
            let reason = "Paused; it can resume from the checkpoint";
            server
                .tasks
                .suspend(&task.id, Some(reason))
                .await
                .map_err(internal)?;
            (Some(reason.to_string()), RunStatus::Suspended)
        } else if result.interrupted {
            let reason = "Interrupted by the user; the task stays where it is";
            server
                .tasks
                .suspend(&task.id, Some(reason))
                .await
                .map_err(internal)?;
            (Some(reason.to_string()), RunStatus::Suspended)
        } else {
            match decision {
                Some(TaskDecision::Done) => {
                    crate::completion_delivery::finish_task(
                        server,
                        &task.id,
                        TaskStatus::Done,
                        None,
                        Some(&result.reply),
                        turn_id,
                        run_id.as_deref(),
                    )
                    .await?;
                    (None, RunStatus::Completed)
                }
                Some(TaskDecision::WaitingForInput(condition_id)) => {
                    let registered = async {
                        let id = run_id
                            .as_deref()
                            .ok_or_else(|| anyhow::anyhow!("Task has no execution identity"))?;
                        let wait = jeeves_state::external_waits::InputWait {
                            task_id: task.id.clone(),
                            condition_id,
                            run_id: id.into(),
                            turn_id: turn_id.clone(),
                            session_id: session_id.clone(),
                            state: "waiting".into(),
                        };
                        crate::task::recovery::require_known_messages(&result.messages)
                            .map_err(|error| anyhow::anyhow!(error.message))?;
                        let messages = serde_json::to_string(&result.messages)?;
                        let usage = serde_json::to_string(&result.usage)?;
                        jeeves_state::external_waits::ExternalWaits::new(server.pool.clone())
                            .register(
                                &wait,
                                jeeves_state::external_waits::WaitSnapshot {
                                    messages_json: &messages,
                                    usage_json: &usage,
                                    iterations: result.iterations,
                                    round_id: &result.round_id,
                                    round_no: *goal_rounds,
                                },
                            )
                            .await
                    }
                    .await;
                    match registered {
                        Ok(()) => {
                            input_wait_committed = true;
                            (
                                Some(jeeves_state::external_waits::INPUT_WAIT_REASON.into()),
                                RunStatus::Waiting,
                            )
                        }
                        Err(error) => {
                            let reason = format!("Cannot persist external input wait: {error}");
                            eprintln!("(jeeves) {reason}");
                            server
                                .tasks
                                .needs_attention(&task.id, &reason)
                                .await
                                .map_err(internal)?;
                            server
                                .checkpoints
                                .finish(turn_id, "suspended")
                                .await
                                .map_err(internal)?;
                            (Some(reason), RunStatus::Waiting)
                        }
                    }
                }
                Some(TaskDecision::WaitingForJobs(job_ids)) => {
                    let waits = jeeves_state::waits::JobWaits::new(server.pool.clone());
                    let registered = match run_id.as_deref() {
                        Some(id) => {
                            waits
                                .register(&task.id, id, turn_id, session_id, &job_ids)
                                .await
                        }
                        None => Err(anyhow::anyhow!("task has no execution identity")),
                    };
                    match registered {
                        Ok(()) => {
                            waiting_for_jobs = true;
                            (
                                Some(jeeves_state::waits::JOB_WAIT_REASON.to_string()),
                                RunStatus::Waiting,
                            )
                        }
                        Err(e) => {
                            let reason = format!("Cannot persist background-job wait: {e}");
                            eprintln!("(jeeves) {reason}");
                            server
                                .tasks
                                .needs_attention(&task.id, &reason)
                                .await
                                .map_err(internal)?;
                            server
                                .checkpoints
                                .finish(turn_id, "suspended")
                                .await
                                .map_err(internal)?;
                            (Some(reason), RunStatus::Waiting)
                        }
                    }
                }
                Some(TaskDecision::NeedsAttention(reason)) => {
                    eprintln!("(jeeves) task {} needs your call: {reason}", task.id);
                    server
                        .tasks
                        .needs_attention(&task.id, &reason)
                        .await
                        .map_err(internal)?;
                    (Some(reason), RunStatus::Waiting)
                }
                Some(TaskDecision::Failed(reason)) => {
                    crate::completion_delivery::finish_task(
                        server,
                        &task.id,
                        TaskStatus::Failed,
                        Some(&reason),
                        None,
                        turn_id,
                        run_id.as_deref(),
                    )
                    .await?;
                    (Some(reason), RunStatus::Failed)
                }
                None => match goal_status.as_deref() {
                    Some("satisfied") => {
                        crate::completion_delivery::finish_task(
                            server,
                            &task.id,
                            TaskStatus::Done,
                            None,
                            Some(&result.reply),
                            turn_id,
                            run_id.as_deref(),
                        )
                        .await?;
                        (None, RunStatus::Completed)
                    }
                    Some(status @ ("impossible" | "round-limit")) => {
                        crate::completion_delivery::finish_task(
                            server,
                            &task.id,
                            TaskStatus::Failed,
                            Some(status),
                            None,
                            turn_id,
                            run_id.as_deref(),
                        )
                        .await?;
                        (Some(status.to_string()), RunStatus::Failed)
                    }
                    Some("blocked") => {
                        let reason = "Judge failure; stopped here";
                        server
                            .tasks
                            .needs_attention(&task.id, reason)
                            .await
                            .map_err(internal)?;
                        (Some(reason.to_string()), RunStatus::Waiting)
                    }
                    Some(_) => {
                        server.tasks.clear_turn(&task.id).await.map_err(internal)?;
                        (None, RunStatus::Running)
                    }
                    None => {
                        if task.goal_condition.is_some() {
                            let reason =
                                "The goal did not advance this round; waiting for the next message";
                            server
                                .tasks
                                .suspend(&task.id, Some(reason))
                                .await
                                .map_err(internal)?;
                            (Some(reason.to_string()), RunStatus::Suspended)
                        } else {
                            crate::completion_delivery::finish_task(
                                server,
                                &task.id,
                                TaskStatus::Done,
                                None,
                                Some(&result.reply),
                                turn_id,
                                run_id.as_deref(),
                            )
                            .await?;
                            (None, RunStatus::Completed)
                        }
                    }
                },
            }
        };
        if let Some(run_id) = &run_id {
            server
                .runs
                .close(run_id, run_status, verdict_reason.as_deref())
                .await
                .map_err(internal)?;
            if let Err(error) = server
                .events
                .append(
                    &task.id,
                    Some(run_id),
                    "run_finished",
                    Some(run_status.as_str()),
                )
                .await
            {
                eprintln!("(jeeves) task run finish event could not be recorded: {error}");
            }
        }
        if let Ok(Some(row)) = server.tasks.get(&task.id).await {
            crate::task::notify(sink, &row, verdict_reason.as_deref());
            if row.status == TaskStatus::Done {
                let queue = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
                match queue.has_ready_dependents(&row.id).await {
                    Ok(true) => crate::task::dispatch::kick(server.clone(), sink.clone()),
                    Ok(false) => (),
                    Err(error) => {
                        eprintln!("(jeeves) task {}: dependent lookup failed: {error}", row.id)
                    }
                }
            }
        }
    } else if let Some(run_id) = &run_id {
        server
            .runs
            .close(run_id, RunStatus::Failed, Some("the task row is gone"))
            .await
            .map_err(internal)?;
    }

    if let Some(reason) = suspended_reason.filter(|_| {
        !wants_job_wait && !wants_input_wait || waiting_for_jobs || input_wait_committed
    }) {
        sink.notification(ServerNotification::TurnSuspended(
            TurnSuspendedNotification {
                turn_id: turn_id.clone(),
                session_id: session_id.clone(),
                reason: reason.to_string(),
            },
        ));
    }
    Ok(Settlement {
        carried_task,
        waiting_for_jobs,
        input_wait_committed,
    })
}
