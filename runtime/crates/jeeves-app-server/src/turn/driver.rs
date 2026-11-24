//! Shared task driver for fresh turns and recovered checkpoints.
use super::history::{judge_transcript, notify_goal};
use super::plan::revise_plan;
use super::{
    FullTurn, RoundCtx, TaskDecision, TurnFrame, account_round, budget_overrun, checkpoint_outlet,
    fail_turn, full_turn, observe_round,
};
use crate::Server;
use jeeves_protocol::ErrorObject;
use jeeves_provider::Resolved;
use jeeves_state::TaskRow;

/// A run that did real work but has no mechanical checks gets one round to compare its result
/// with the original request before it is delivered. A self-report is still not a measurement:
/// the output stays unverified, but obvious gaps are caught while the work is still open.
const SELF_CHECK: &str = "Self-check before delivery: re-read the original assignment above and check every requirement against what you actually produced. Open the files you wrote and rerun what can be rerun. Fix anything missing or wrong now. Then give the person the final result, and say plainly what you could not verify. Do not claim checks you did not run.";

pub(crate) struct DriveTask<'a> {
    pub server: &'a Server,
    pub resolved: &'a Resolved,
    pub task: Option<TaskRow>,
    pub assignment: &'a str,
    pub run_id: Option<String>,
    pub frame: TurnFrame<'a>,
    pub plan: Option<jeeves_agent::plan::Plan>,
    pub round_no: i32,
    pub max_rounds: i32,
    pub started: std::time::Instant,
    pub wall_offset_ms: i64,
}

pub(crate) struct DrivenTask {
    pub full: FullTurn,
    pub decision: TaskDecision,
    pub goal_status: Option<String>,
    pub round_no: i32,
}

pub(crate) async fn drive_task(
    ctx: DriveTask<'_>,
    mut full: FullTurn,
) -> Result<DrivenTask, ErrorObject> {
    let DriveTask {
        server,
        resolved,
        task,
        assignment,
        run_id,
        mut frame,
        mut plan,
        mut round_no,
        max_rounds,
        started,
        mut wall_offset_ms,
    } = ctx;
    let TurnFrame {
        session_id,
        turn_id,
        sink,
        turn_cwd,
        ..
    } = frame.clone();
    let (session_id, turn_id) = (session_id.to_string(), turn_id.to_string());
    let checkpoint_for_round = |round| {
        Some(checkpoint_outlet(
            server,
            sink,
            &turn_id,
            &session_id,
            assignment,
            run_id.as_deref(),
            round,
        ))
    };
    let planner = super::meter::metered(
        &server.pool,
        resolved,
        &session_id,
        &turn_id,
        run_id.as_deref(),
        "planning",
    );
    let judge = super::meter::metered(
        &server.pool,
        resolved,
        &session_id,
        &turn_id,
        run_id.as_deref(),
        "evaluation",
    );
    let task_id = task.as_ref().map(|task| task.id.clone());
    let goal_condition = task.as_ref().and_then(|task| task.goal_condition.clone());
    let acceptance = match task
        .as_ref()
        .map(|task| jeeves_agent::verify::parse_checked(&task.acceptance_json))
        .transpose()
    {
        Ok(acceptance) => acceptance.unwrap_or_default(),
        Err(reason) => {
            return Ok(DrivenTask {
                full,
                decision: TaskDecision::NeedsAttention(reason),
                goal_status: Some("invalid-contract".into()),
                round_no,
            });
        }
    };
    if let Some(condition) = &goal_condition {
        eprintln!("(jeeves) goal loop starts: {condition} (max {max_rounds} rounds)");
    }
    let mut round_started = started;
    let mut self_checked = false;
    let (decision, goal_status) = loop {
        let observation =
            match observe_round(server, &turn_id, &full.result.round_id, turn_cwd.as_deref()).await
            {
                Ok(value) => value,
                Err(reason) => {
                    break (
                        TaskDecision::NeedsAttention(reason),
                        Some("accounting-failed".into()),
                    );
                }
            };
        let goal_rounds = round_no;
        if let Some(id) = &task_id
            && let Err(e) = server.tasks.record_round(id, round_no).await
        {
            break (
                TaskDecision::NeedsAttention(format!("Cannot record task round: {e}")),
                Some("accounting-failed".into()),
            );
        }

        let checks = match account_round(
            server,
            RoundCtx {
                tool_ctx: server.tool_ctx(
                    &session_id,
                    turn_cwd.clone(),
                    server.session_approval(&session_id, &turn_id, sink),
                ),
                task: task.as_ref(),
                run_id: run_id.as_deref(),
                acceptance: &acceptance,
                verify_root: turn_cwd.clone(),
                started: round_started,
                wall_offset_ms,
            },
            &observation,
            &full.result,
        )
        .await
        {
            Ok(checks) => checks,
            Err(reason) => {
                eprintln!("(jeeves) {reason}; task stopped for review");
                break (
                    TaskDecision::NeedsAttention(reason),
                    Some("accounting-failed".to_string()),
                );
            }
        };

        if full.result.interrupted {
            eprintln!("(jeeves) interrupted; the goal loop stops at this round");
            break (
                TaskDecision::NeedsAttention(
                    "Interrupted by the user; the task stays where it is".to_string(),
                ),
                Some("interrupted".to_string()),
            );
        }

        if let Some(id) = &task_id {
            match server.tasks.get(id).await {
                Ok(Some(fresh)) => {
                    if let Some(over) = budget_overrun(&fresh) {
                        eprintln!("(jeeves) task {id} stopped on its budget: {over}");
                        if let Err(e) = server
                            .events
                            .append(id, run_id.as_deref(), "budget", Some(&over))
                            .await
                        {
                            eprintln!("(jeeves) budget event not recorded: {e}");
                        }
                        break (
                            TaskDecision::NeedsAttention(over),
                            Some("over-budget".to_string()),
                        );
                    }
                }
                Ok(None) => {
                    break (
                        TaskDecision::NeedsAttention("Task disappeared during execution".into()),
                        Some("task-unavailable".into()),
                    );
                }
                Err(e) => {
                    break (
                        TaskDecision::NeedsAttention(format!("Cannot verify task budget: {e}")),
                        Some("budget-unavailable".into()),
                    );
                }
            }
        }

        if full.result.stop_reason == jeeves_provider::StopReason::UncertainEffect {
            break (
                TaskDecision::NeedsAttention(
                    "Tool outcome unknown; reconcile external state before continuing".into(),
                ),
                Some("unknown-effect".into()),
            );
        }

        if let Some(task) = &task {
            match jeeves_state::Jobs::new(server.pool.clone())
                .for_task(&task.id)
                .await
            {
                Ok(jobs) if jobs.iter().any(|job| job.status == "running") => {
                    eprintln!("(jeeves) task {} is waiting for background jobs", task.id);
                    break (
                        TaskDecision::WaitingForJobs(
                            jobs.into_iter()
                                .filter(|job| job.status == "running")
                                .map(|job| job.id)
                                .collect(),
                        ),
                        Some("waiting-jobs".into()),
                    );
                }
                Ok(jobs)
                    if frame.worktree.is_some()
                        && jobs.iter().any(|job| {
                            job.run_id.as_deref() == run_id.as_deref()
                                && matches!(
                                    job.status.as_str(),
                                    "failed" | "interrupted" | "killed"
                                )
                        }) =>
                {
                    break (TaskDecision::NeedsAttention("Coding executor did not settle successfully; review the job and worktree before continuing".into()), Some("coding-job-review".into()));
                }
                Ok(_) => {}
                Err(e) => {
                    break (
                        TaskDecision::NeedsAttention(format!("Cannot verify background jobs: {e}")),
                        Some("jobs-unavailable".into()),
                    );
                }
            }
        }

        if let Some(task) = &task {
            let inputs = jeeves_state::external_waits::ExternalWaits::new(server.pool.clone());
            match crate::task::inputs::awaiting(&inputs, &task.id, run_id.as_deref()).await {
                Ok(Some(input)) => match inputs.expired(&task.id).await {
                    Ok(false) => {
                        break (
                            TaskDecision::WaitingForInput(input.condition_id),
                            Some("waiting-input".into()),
                        );
                    }
                    Ok(true) => {
                        break (
                            TaskDecision::NeedsAttention(
                                "Expected input did not arrive before its deadline".into(),
                            ),
                            Some("input-expired".into()),
                        );
                    }
                    Err(error) => {
                        break (
                            TaskDecision::NeedsAttention(format!(
                                "Cannot verify task input deadline: {error}"
                            )),
                            Some("input-unavailable".into()),
                        );
                    }
                },
                Ok(None) => (),
                Err(error) => {
                    break (
                        TaskDecision::NeedsAttention(format!(
                            "Cannot verify task input condition: {error}"
                        )),
                        Some("input-unavailable".into()),
                    );
                }
            }
        }

        let response_limit = if let Some(reason) = full.result.stop_detail.as_deref() {
            Some(reason)
        } else if full.result.token_budget_reached && task.is_some() {
            Some("The turn reached its token budget")
        } else {
            match full.result.stop_reason {
                jeeves_provider::StopReason::MaxTokens => {
                    Some("Model output reached its token limit")
                }
                _ => None,
            }
        };
        if let Some(limit) = response_limit {
            break (
                TaskDecision::NeedsAttention(format!("{limit}. Task completion is not verified.")),
                Some("response-limit".into()),
            );
        }

        if let Some(reason) = jeeves_agent::verify::failure_summary(&checks) {
            let who = task_id.as_deref().unwrap_or("(no task)");
            if round_no >= max_rounds {
                let reason = format!("{reason} (after {round_no} rounds it still has not passed)");
                eprintln!("(jeeves) task {who} stopped at acceptance: {reason}");
                break (
                    TaskDecision::NeedsAttention(reason),
                    Some("verify-failed".to_string()),
                );
            }
            eprintln!("(jeeves) task {who} failed acceptance on round {round_no}: {reason}");
            round_no += 1;
            frame.checkpoint = checkpoint_for_round(round_no);
            round_started = std::time::Instant::now();
            wall_offset_ms = 0;
            plan = revise_plan(server, &planner, task.as_ref(), &acceptance, plan, &reason).await;
            let correction = next_round_message(
                assignment,
                &format!("Acceptance did not pass: {reason}"),
                plan.as_ref(),
            );
            let next = full_turn(
                server,
                resolved,
                &correction,
                Vec::new(),
                frame.clone(),
                None,
            )
            .await;
            match next {
                Ok(next) => {
                    full = next;
                    continue;
                }
                Err(e) => {
                    fail_turn(
                        server,
                        sink,
                        &turn_id,
                        run_id.as_deref(),
                        task_id.as_deref(),
                        &e.message,
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        if let Some(contradiction) = observation.contradictions().first() {
            eprintln!(
                "(jeeves) task {} stopped on a contradiction: {contradiction}",
                task_id.as_deref().unwrap_or("(no task)")
            );
            break (
                TaskDecision::NeedsAttention(contradiction.clone()),
                Some("contradiction".to_string()),
            );
        }

        let worked = full.result.tool_calls.iter().any(|call| call.ok());
        if goal_condition.is_none()
            && acceptance.is_empty()
            && worked
            && !self_checked
            && round_no < max_rounds
            && let Some(id) = &task_id
        {
            self_checked = true;
            eprintln!("(jeeves) task {id}: one self-check round before delivery");
            if let Err(e) = server
                .events
                .append(
                    id,
                    run_id.as_deref(),
                    "self_check",
                    Some("review against the request before delivery"),
                )
                .await
            {
                eprintln!("(jeeves) self-check event not recorded: {e}");
            }
            round_no += 1;
            frame.checkpoint = checkpoint_for_round(round_no);
            round_started = std::time::Instant::now();
            wall_offset_ms = 0;
            let review = next_round_message(assignment, SELF_CHECK, plan.as_ref());
            match full_turn(server, resolved, &review, Vec::new(), frame.clone(), None).await {
                Ok(next) => {
                    full = next;
                    continue;
                }
                Err(e) => {
                    fail_turn(
                        server,
                        sink,
                        &turn_id,
                        run_id.as_deref(),
                        Some(id),
                        &e.message,
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        let Some(condition) = &goal_condition else {
            break (
                TaskDecision::Done,
                if checks.is_empty() {
                    None
                } else {
                    Some("verified".to_string())
                },
            );
        };

        let transcript = judge_transcript(server, &session_id, &full.result.reply).await;
        let judgement = match crate::goal::judge(&judge, condition, &transcript).await {
            Ok(judgement) => judgement,
            Err(why) => {
                notify_goal(sink, &turn_id, goal_rounds, max_rounds, "blocked", &why);
                eprintln!("(jeeves) goal loop stopped: {why}");
                break (
                    TaskDecision::NeedsAttention(format!("Judge failure: {why}")),
                    Some("blocked".to_string()),
                );
            }
        };

        if judgement.ok {
            notify_goal(
                sink,
                &turn_id,
                goal_rounds,
                max_rounds,
                "satisfied",
                &judgement.reason,
            );
            break (TaskDecision::Done, Some("satisfied".to_string()));
        }
        if judgement.impossible {
            notify_goal(
                sink,
                &turn_id,
                goal_rounds,
                max_rounds,
                "impossible",
                &judgement.reason,
            );
            break (
                TaskDecision::Failed(judgement.reason),
                Some("impossible".to_string()),
            );
        }
        if round_no >= max_rounds {
            let reason = format!(
                "After {max_rounds} more rounds the goal is still unmet: {}",
                judgement.reason
            );
            notify_goal(
                sink,
                &turn_id,
                goal_rounds,
                max_rounds,
                "round-limit",
                &reason,
            );
            break (
                TaskDecision::Failed(reason),
                Some("round-limit".to_string()),
            );
        }

        notify_goal(
            sink,
            &turn_id,
            goal_rounds,
            max_rounds,
            "continuing",
            &judgement.reason,
        );
        eprintln!(
            "(jeeves) goal loop round {goal_rounds} of {max_rounds}: {}",
            judgement.reason
        );
        round_no += 1;
        frame.checkpoint = checkpoint_for_round(round_no);
        round_started = std::time::Instant::now();
        wall_offset_ms = 0;
        let continuation = next_round_message(assignment, &judgement.reason, plan.as_ref());
        let next = full_turn(
            server,
            resolved,
            &continuation,
            Vec::new(),
            frame.clone(),
            None,
        )
        .await;
        match next {
            Ok(next) => {
                full = next;
            }
            Err(e) => {
                fail_turn(
                    server,
                    sink,
                    &turn_id,
                    run_id.as_deref(),
                    task_id.as_deref(),
                    &e.message,
                )
                .await;
                return Err(e);
            }
        }
    };

    Ok(DrivenTask {
        full,
        decision,
        goal_status,
        round_no,
    })
}

fn next_round_message(
    assignment: &str,
    feedback: &str,
    plan: Option<&jeeves_agent::plan::Plan>,
) -> String {
    let mut message = format!(
        "Original assignment:\n{assignment}\n\nContinue from recorded progress, respecting subsequent human decisions. Verify the remaining work before reporting completion.\nRound feedback: {feedback}"
    );
    if let Some(plan) = plan {
        message.push_str("\n\n");
        message.push_str(&plan.brief());
    }
    message
}
