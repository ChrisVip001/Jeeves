//! Startup reconciliation — the crash-recovery half of the long-task engine.
//!
//! A process that died mid-turn leaves its checkpoint `open` in state.db. At startup those
//! turns resume automatically: the human asked for this work by starting the turn, and the
//! goal loop's rule already covers the spirit — continuing is not the model assigning itself
//! work. Finished tools are never re-run; the loop continues from the checkpointed boundary.
//!
//! What is **not** resumed: turns the human interrupted, turns that failed inside their
//! process (resuming a failed model call fails the same way again), and turns older than
//! `JEEVES_RESUME_MAX_AGE` (default one hour) — all three are `abandoned`, each with one line
//! on stderr. New tool executions still pass the approval gate (default-deny) — resuming
//! re-enters approved territory, it never widens it.
//!
//! The age limit is not caution for its own sake. A checkpoint exists for crash recovery,
//! so its honest window is seconds to minutes; a turn left `open` for two days is not "the
//! same turn continued". "Give me today's tech news" answered two days later answers a
//! question nobody is asking any more, and it spends a real model call plus a real fetch to
//! do it. Asking again costs one sentence.

use jeeves_protocol::{ServerNotification, TurnResumedNotification, TurnStartedNotification};
use jeeves_provider::{ContentBlock, Message, Resolved, Role, Usage};
use jeeves_state::CheckpointRow;
use jeeves_state::checkpoints::CheckpointWrite;
use jeeves_state::operations::OperationSnapshot;
use std::sync::Arc;

use crate::turn::{FinishedTurn, TurnFrame, TurnGuard, finalize_turn, full_turn};
use crate::{EventSink, Server};

/// Resume every `open` checkpoint. Called once per transport start; each resume runs as its
/// own task, so one broken turn never blocks the others (or the request loop).
pub async fn reconcile(server: &Server, sink: &EventSink) {
    if let Err(error) = server.usage.charge_pending(None, None).await {
        eprintln!("(jeeves) cannot recover reported model usage; recovery stopped: {error}");
        return;
    }
    let rows = match server.checkpoints.open_rows().await {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!(
                "(jeeves) checkpoint reconciliation could not read the database (nothing resumes, startup is not blocked): {e}"
            );
            return;
        }
    };
    let max_age = server.settings().resume_max_age_minutes;
    let now = chrono::Utc::now().naive_utc();
    for row in rows {
        if let Err(reason) = crate::turn::restore_observation(server, &row).await {
            crate::task::contract::stop_recovery(server, sink, &row, &reason).await;
            continue;
        }
        if row.run_id.is_some() && row.round_id.is_some() {
            let charged = match crate::task::contract::checkpoint_usage(&row.usage_json) {
                Ok(usage) => crate::task::contract::charge_checkpoint(server, &row, usage).await,
                Err(error) => Err(error),
            };
            if let Err(error) = charged {
                crate::task::contract::stop_recovery(
                    server,
                    sink,
                    &row,
                    &format!("Checkpoint usage could not be charged: {error}"),
                )
                .await;
                continue;
            }
        }
        if let Some(age) = age_minutes(&row.updated_at, now)
            && max_age > 0
            && age > i64::from(max_age)
            && !crate::resume_task::exempt_from_age(server, &row).await
        {
            eprintln!(
                "(jeeves) reconcile: turn {} stopped before {} (limit {}), not resuming on its own — to carry on, say that sentence again",
                row.turn_id,
                humanize(age),
                humanize(i64::from(max_age)),
            );
            if let Err(error) = server.checkpoints.finish(&row.turn_id, "abandoned").await {
                eprintln!(
                    "(jeeves) reconcile: turn {} could not be marked abandoned: {error}",
                    row.turn_id
                );
            }
            continue;
        }
        eprintln!(
            "(jeeves) reconcile: found unfinished turn {} ({} rounds done), resuming automatically",
            row.turn_id, row.iterations
        );
        let server = server.clone();
        let sink = sink.clone();
        tokio::spawn(async move { resume_turn(&server, &sink, row, None).await });
    }
}

/// How long ago the checkpoint was written, in minutes — `None` when the stamp is unreadable.
///
/// `updated_at` comes from SQLite's `datetime('now')`: UTC, second resolution, no zone
/// marker. A stamp we cannot parse is **not** treated as stale: refusing to resume costs
/// the human work they asked for, while resuming is exactly what the old rule did. Unknown
/// is a reason to be loud on stderr, not a reason to throw a turn away.
pub(crate) fn age_minutes(updated_at: &str, now: chrono::NaiveDateTime) -> Option<i64> {
    let at = chrono::NaiveDateTime::parse_from_str(updated_at.trim(), "%Y-%m-%d %H:%M:%S").ok()?;
    Some((now - at).num_minutes())
}

/// 90 → `1 hour`, 3000 → `2 days`. Rounded down on purpose: "2 days ago" must not read as 1.
pub(crate) fn humanize(minutes: i64) -> String {
    if minutes >= 1_440 {
        format!("{} days", minutes / 1_440)
    } else if minutes >= 60 {
        format!("{} hours", minutes / 60)
    } else {
        format!("{} minutes", minutes.max(0))
    }
}

/// Continue a paused or stopped checkpoint on explicit human authorization (`task/resume`).
///
/// This is the other half of reconcile: crash recovery resumes what the process left `open`
/// (nobody asked), while a pause or stop waits for explicit human continuation.
/// No suspended checkpoint for that turn → nothing to continue (the task then waits for
/// its next message instead) — said out loud, not silently skipped.
pub(crate) fn resume_authorized(server: &Server, sink: &EventSink, row: CheckpointRow) {
    let server = server.clone();
    let sink = sink.clone();
    let turn_id = row.turn_id.clone();
    tokio::spawn(async move {
        eprintln!("(jeeves) task resuming from where it paused: turn {turn_id}");
        resume_turn(&server, &sink, row, None).await;
    });
}

pub(crate) async fn resume_waiting(
    server: &Server,
    sink: &EventSink,
    wait: jeeves_state::waits::JobWait,
    jobs: &[jeeves_state::JobRow],
) -> bool {
    let mut row = match server.checkpoints.get(&wait.turn_id).await {
        Ok(Some(row)) if row.status == "waiting_jobs" => row,
        Ok(_) => return false,
        Err(e) => {
            eprintln!("(jeeves) waiting checkpoint unavailable: {e}");
            return false;
        }
    };
    let messages = serde_json::from_str::<Vec<Message>>(&row.messages_json)
        .map_err(|_| "Waiting checkpoint has unreadable working memory".to_string())
        .and_then(|messages| {
            crate::task::recovery::require_known_messages(&messages)
                .map_err(|e| e.message)
                .map(|_| messages)
        });
    let mut messages = match messages {
        Ok(messages) => messages,
        Err(reason) => return crate::task::waits::review(server, sink, &wait, &reason).await,
    };
    let summary = jobs
        .iter()
        .map(|job| {
            format!(
                "{}: {}, exit {:?}; output {}",
                job.id,
                job.status,
                job.exit_code,
                job.log_path.as_deref().unwrap_or("unavailable")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    messages.push(Message::user_text(format!("The background jobs for this authorized task have finished. Review the actual output and verify the task before claiming completion. Do not relaunch a completed command merely because this turn resumed.\n{summary}")));
    row.messages_json = match serde_json::to_string(&messages) {
        Ok(json) => json,
        Err(_) => return false,
    };
    resume_turn(server, sink, row, Some(crate::wake::Wake::Jobs(wait))).await
}

/// What the model reads for a read-only call the crash cut off. `Error:` settles the operation
/// as failed, which is the truth: no result was observed.
const INTERRUPTED_READ: &str = "Error: the process stopped before this read-only call returned. Nothing was changed; call it again if you still need the result.";

/// Reconstruct a committed local handoff or close an interrupted read. Any other missing
/// result stays unresolved. Persist the message and operation status together.
async fn settle_known_calls(
    server: &Server,
    row: &mut CheckpointRow,
    messages: &mut Vec<Message>,
) -> anyhow::Result<usize> {
    let pending =
        crate::task::recovery::pending_tools(messages).map_err(|e| anyhow::anyhow!(e.message))?;
    let mut settled = Vec::new();
    for (id, (tool, input)) in pending {
        let output = if jeeves_tools::permissions::proven_read_call(tool, input) {
            jeeves_state::operations::ToolOutput::failure(INTERRUPTED_READ.into())
        } else if matches!(
            tool,
            crate::interaction::task_tool::NAME | "selected_price_watch"
        ) {
            let Some(receipt) =
                crate::interaction::handoff::committed_result(server, row, id, input).await?
            else {
                return Ok(0);
            };
            jeeves_state::operations::ToolOutput::success(receipt)
        } else {
            return Ok(0);
        };
        settled.push((id.to_owned(), tool.to_owned(), input.clone(), output));
    }
    if settled.is_empty() {
        return Ok(0);
    }
    let results = settled
        .iter()
        .map(|(id, _, _, output)| ContentBlock::ToolResult {
            tool_use_id: id.clone(),
            content: output.text.clone(),
        });
    match messages.last_mut() {
        Some(last) if last.role == Role::User => last.content.extend(results),
        _ => messages.push(Message {
            role: Role::User,
            content: results.collect(),
        }),
    }
    if let [.., proposal, response] = messages.as_mut_slice() {
        response.content.sort_by_cached_key(|result| match result {
            ContentBlock::ToolResult { tool_use_id, .. } => proposal
                .content
                .iter()
                .position(
                    |block| matches!(block, ContentBlock::ToolUse { id, .. } if id == tool_use_id),
                )
                .unwrap_or(usize::MAX),
            _ => usize::MAX,
        });
    }
    let messages_json = serde_json::to_string(messages)?;
    let operations: Vec<OperationSnapshot> = match row.round_id {
        Some(_) => settled
            .iter()
            .map(|(id, name, input, output)| OperationSnapshot {
                proposed_only: false,
                tool_use_id: id.clone(),
                tool: name.clone(),
                input_json: input.to_string(),
                output: Some(output.clone()),
            })
            .collect(),
        None => Vec::new(),
    };
    server
        .checkpoints
        .save_with_operations(
            CheckpointWrite {
                turn_id: &row.turn_id,
                session_id: &row.session_id,
                user_message: &row.user_message,
                messages_json: &messages_json,
                iterations: row.iterations,
                usage_json: &row.usage_json,
                run_id: row.run_id.as_deref(),
                round_id: row.round_id.as_deref(),
                round_no: row.round_no,
            },
            &operations,
        )
        .await?;
    row.messages_json = messages_json;
    Ok(settled.len())
}

/// Continue one checkpointed turn and give it the same accounting as a fresh one.
pub(crate) async fn resume_turn(
    server: &Server,
    sink: &EventSink,
    row: CheckpointRow,
    waiting: Option<crate::wake::Wake>,
) -> bool {
    if row.run_id.is_none() {
        match server.checkpoints.is_chat_turn(&row.turn_id).await {
            Ok(true) => (),
            Ok(false) => {
                crate::task::contract::stop_recovery(server, sink, &row,
                    "Task checkpoint is missing its execution identity; review it before continuing").await;
                return false;
            }
            Err(e) => {
                eprintln!(
                    "(jeeves) cannot verify checkpoint ownership for {}: {e}",
                    row.turn_id
                );
                return false;
            }
        }
    }
    let mut row = row;
    let Ok(mut messages) = serde_json::from_str::<Vec<Message>>(&row.messages_json) else {
        eprintln!(
            "(jeeves) the working memory of checkpoint {} could not be read (schema drift?), marked abandoned",
            row.turn_id
        );
        let _ = server.checkpoints.finish(&row.turn_id, "abandoned").await;
        return false;
    };
    if row.status == "open" {
        match settle_known_calls(server, &mut row, &mut messages).await {
            Ok(0) => {}
            Ok(count) => eprintln!(
                "(jeeves) reconcile: {count} call(s) of turn {} settled from read-only status or durable handoff evidence",
                row.turn_id
            ),
            Err(error) => {
                crate::task::contract::stop_recovery(
                    server,
                    sink,
                    &row,
                    &format!("Interrupted calls could not be settled: {error}"),
                )
                .await;
                return false;
            }
        }
    }
    if crate::task::recovery::require_known_messages(&messages).is_err() {
        crate::task::contract::stop_recovery(
            server,
            sink,
            &row,
            "Tool outcome unknown after interruption; reconcile before retrying",
        )
        .await;
        return false;
    }
    let mut usage: Usage = match crate::task::contract::checkpoint_usage(&row.usage_json) {
        Ok(usage) => usage,
        Err(error) => {
            crate::task::contract::stop_recovery(server, sink, &row, &error).await;
            return false;
        }
    };
    let resolved: Resolved = match server.resolved() {
        Some(resolved) => resolved,
        None => {
            if row.round_id.is_some()
                && let Err(error) =
                    crate::task::contract::charge_checkpoint(server, &row, usage).await
            {
                crate::task::contract::stop_recovery(
                    server,
                    sink,
                    &row,
                    &format!("Checkpoint usage could not be charged: {error}"),
                )
                .await;
                return false;
            }
            eprintln!(
                "(jeeves) keeping checkpoint {} for now: no model is configured; it resumes at the next start once there is one",
                row.turn_id
            );
            return false;
        }
    };
    let session_id = row.session_id.clone();
    let turn_id = row.turn_id.clone();

    if row.round_id.is_none()
        && let Some(id) = row.run_id.as_deref()
        && let Ok(Some(run)) = server.runs.get(id).await
        && run.rounds > 0
    {
        let reason =
            "Legacy checkpoint has no round accounting identity; review spend before resuming";
        let _ = server.tasks.needs_attention(&run.task_id, reason).await;
        let _ = server.checkpoints.finish(&row.turn_id, "suspended").await;
        eprintln!("(jeeves) {}: {reason}", row.turn_id);
        return false;
    }
    let task = match row.run_id.as_deref() {
        Some(id) => match server.runs.get(id).await {
            Ok(Some(run)) => match server.tasks.get(&run.task_id).await {
                Ok(Some(task)) if task.status.is_open() => Some(task),
                _ => {
                    eprintln!("(jeeves) resume stopped: the task of run {id} is unavailable");
                    return false;
                }
            },
            _ => {
                eprintln!("(jeeves) resume stopped: run {id} was not found");
                return false;
            }
        },
        None => None,
    };
    let background_lock = match &task {
        Some(task) => match jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
            .get(&task.id)
            .await
        {
            Ok(Some(_)) => Some(server.session_write_lock("background-task-worker")),
            Ok(None) => None,
            Err(error) => {
                eprintln!("(jeeves) cannot read recovered task dispatch: {error}");
                return false;
            }
        },
        None => None,
    };
    let _background_guard = match &background_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let (interrupt, steering) = match server.begin_turn(&turn_id, &session_id) {
        Ok(pair) => pair,
        Err(holder) => {
            eprintln!(
                "(jeeves) session {session_id} already has {holder} running; checkpoint {turn_id} is left for the next start"
            );
            return false;
        }
    };
    let _guard = TurnGuard {
        server,
        turn_id: turn_id.clone(),
        session_id: session_id.clone(),
    };

    let control = task
        .as_ref()
        .map(|task| server.session_write_lock(&format!("task-control:{}", task.id)));
    let control_guard = match control.as_ref() {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    if let Some(task) = &task {
        match jeeves_state::human_pauses::HumanPauses::new(server.pool.clone())
            .is_paused(&task.id)
            .await
        {
            Ok(false) => (),
            Ok(true) => {
                let _ = server.checkpoints.finish(&turn_id, "suspended").await;
                eprintln!(
                    "(jeeves) task {} remains paused by the user; automatic recovery was refused",
                    task.id
                );
                crate::task::notify(
                    sink,
                    task,
                    Some("Paused by the user; explicit continuation is required"),
                );
                return false;
            }
            Err(error) => {
                eprintln!("(jeeves) human pause could not be verified; recovery stopped: {error}");
                return false;
            }
        }
        match crate::browser::is_held(&server.pool, &task.id).await {
            Ok(false) => (),
            Ok(true) => {
                let _ = server.checkpoints.finish(&turn_id, "suspended").await;
                eprintln!(
                    "(jeeves) task {} is held: the human was in control of its browser when the Runtime stopped; automatic recovery was refused",
                    task.id
                );
                crate::task::notify(
                    sink,
                    task,
                    Some(
                        "Held while you were in control of the browser; open a fresh browser to continue",
                    ),
                );
                return false;
            }
            Err(error) => {
                eprintln!("(jeeves) browser hold could not be verified; recovery stopped: {error}");
                return false;
            }
        }
        match server.runs.list_by_task(&task.id, 1).await {
            Ok(runs) if runs.first().map(|run| run.id.as_str()) == row.run_id.as_deref() => (),
            Ok(_) => {
                if let Err(error) = server.checkpoints.finish(&turn_id, "abandoned").await {
                    eprintln!("(jeeves) stale checkpoint could not be closed: {error}");
                }
                if let Some(id) = &row.run_id
                    && let Err(error) = server.runs.supersede(id).await
                {
                    eprintln!("(jeeves) stale run could not be suspended: {error}");
                }
                eprintln!(
                    "(jeeves) checkpoint {turn_id} belongs to an older execution; it will not resume"
                );
                return false;
            }
            Err(error) => {
                eprintln!(
                    "(jeeves) latest execution could not be verified; recovery stopped: {error}"
                );
                return false;
            }
        }
        match server.tasks.get(&task.id).await {
            Ok(Some(current)) if current.status.is_open() => {}
            _ => {
                eprintln!("(jeeves) recovered task is no longer open; execution stopped");
                return false;
            }
        }
        let expected = waiting
            .as_ref()
            .map(crate::wake::Wake::checkpoint_status)
            .unwrap_or("open");
        match server.checkpoints.get(&turn_id).await {
            Ok(Some(current)) if current.status == expected => {}
            _ => {
                eprintln!("(jeeves) recovered checkpoint admission changed; execution stopped");
                return false;
            }
        }
    }
    if let Some(task) = &task {
        let admission = match crate::task::contract::charge_checkpoint(server, &row, usage).await {
            Ok(()) => match server.tasks.get(&task.id).await {
                Ok(Some(current)) => crate::task::contract::admit(server, sink, &current)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.message),
                _ => Err("Recovered task is unavailable".into()),
            },
            Err(error) => Err(format!("Checkpoint usage could not be charged: {error}")),
        };
        if let Err(reason) = admission {
            crate::task::contract::stop_recovery(server, sink, &row, &reason).await;
            return false;
        }
    }
    if let Some(wait) = &waiting {
        match wait.claim(server, &row.messages_json).await {
            Ok(true) => eprintln!(
                "(jeeves) task {} input dependencies settled; resuming authorized work",
                wait.task_id()
            ),
            Ok(false) => return false,
            Err(error) => {
                eprintln!("(jeeves) durable wait could not be claimed: {error}");
                return false;
            }
        }
    }
    let run_id = row.run_id.clone();
    let round_no = row.round_no.unwrap_or_else(|| {
        task.as_ref()
            .map(|task| task.rounds_used.max(1))
            .unwrap_or(1)
    });
    let round_id = row
        .round_id
        .clone()
        .unwrap_or_else(|| format!("legacy:{}", row.turn_id));
    let wall_offset_ms = match run_id.as_deref() {
        Some(id) => match server.runs.round_account(id, &round_id).await {
            Ok(account) => {
                usage.input_tokens = usage.input_tokens.max(account.tokens_in);
                usage.output_tokens = usage.output_tokens.max(account.tokens_out);
                account.wall_ms
            }
            Err(e) => {
                crate::task::contract::stop_recovery(
                    server,
                    sink,
                    &row,
                    &format!("Cannot read recovered round accounting: {e}"),
                )
                .await;
                return false;
            }
        },
        None => 0,
    };
    let plan = match &task {
        Some(task) => match jeeves_agent::plan::restore(&server.steps, &task.id).await {
            Ok(plan) => plan,
            Err(error) => {
                crate::task::contract::stop_recovery(
                    server,
                    sink,
                    &row,
                    &format!("Cannot restore task plan: {error}"),
                )
                .await;
                return false;
            }
        },
        None => None,
    };
    if let Some(run_id) = &run_id {
        match server.runs.reopen(run_id).await {
            Ok(true) => {}
            outcome => {
                let reason = format!("Cannot reopen checkpoint run {run_id}: {outcome:?}");
                crate::task::contract::stop_recovery(server, sink, &row, &reason).await;
                return false;
            }
        }
    }

    if let Some(task) = &task {
        if let Err(error) = server.tasks.set_running(&task.id, &turn_id).await {
            crate::task::contract::stop_recovery(
                server,
                sink,
                &row,
                &format!("Recovered task activation failed: {error}"),
            )
            .await;
            return false;
        }
        if let Ok(Some(row)) = server.tasks.get(&task.id).await {
            crate::task::notify(sink, &row, Some("turn resumed"));
        }
    }
    sink.notification(ServerNotification::TurnStarted(TurnStartedNotification {
        turn_id: turn_id.clone(),
        session_id: session_id.clone(),
        user_message: row.user_message.clone(),
        ts: chrono::Local::now().to_rfc3339(),
    }));
    sink.notification(ServerNotification::TurnResumed(TurnResumedNotification {
        turn_id: turn_id.clone(),
        session_id: session_id.clone(),
        from_iterations: row.iterations,
        reason: "reconcile: crash recovery".to_string(),
    }));

    drop(control_guard);

    let started = std::time::Instant::now();
    let cwd = task
        .as_ref()
        .and_then(|task| task.cwd.as_deref())
        .map(std::path::PathBuf::from);
    let frame = TurnFrame {
        interaction_tool: None,
        session_id: &session_id,
        turn_id: &turn_id,
        sink,
        interrupt: Some(interrupt.clone()),
        worktree: crate::task::worktree::owned(&server.settings().home, task.as_ref()),
        turn_cwd: cwd,
        tool_limits: Arc::new(jeeves_tools::web::TurnLimits::from_env()),
        resume: Some(jeeves_loop::Resume {
            round_id: round_id.clone(),
            messages,
            completed_iterations: row.iterations,
            usage,
        }),
        checkpoint: Some(crate::turn::checkpoint_outlet(
            server,
            sink,
            &turn_id,
            &session_id,
            &row.user_message,
            run_id.as_deref(),
            round_no,
        )),
        steering: Some(steering.clone()),
    };

    match full_turn(
        server,
        &resolved,
        &row.user_message,
        Vec::new(),
        frame.clone(),
        None,
    )
    .await
    {
        Ok(full) => {
            let driven = crate::turn::driver::drive_task(
                crate::turn::driver::DriveTask {
                    server,
                    resolved: &resolved,
                    task: task.clone(),
                    assignment: &row.user_message,
                    run_id: run_id.clone(),
                    frame: TurnFrame {
                        resume: None,
                        ..frame
                    },
                    plan,
                    round_no,
                    max_rounds: task
                        .as_ref()
                        .map(|task| task.max_rounds.max(1))
                        .unwrap_or(1),
                    started,
                    wall_offset_ms,
                },
                full,
            )
            .await;
            let driven = match driven {
                Ok(driven) => driven,
                Err(error) => {
                    eprintln!("(jeeves) recovered task driver failed: {}", error.message);
                    return false;
                }
            };
            let full = driven.full;
            let outcome = finalize_turn(
                server,
                sink,
                FinishedTurn {
                    session_id,
                    turn_id,
                    user_message: &row.user_message,
                    resolved: &resolved,
                    result: full.result,
                    route_reason: None,
                    retries: 0,
                    goal_status: driven.goal_status,
                    goal_rounds: driven.round_no,
                    history: full.history,
                    started,
                    run_id,
                    decision: task.as_ref().map(|_| driven.decision),
                },
            )
            .await;
            if let Err(e) = outcome {
                eprintln!(
                    "(jeeves) the wrap-up of the resumed turn failed: {}",
                    e.message
                );
            }
        }
        Err(e) => {
            crate::turn::fail_turn(
                server,
                sink,
                &turn_id,
                run_id.as_deref(),
                task.as_ref().map(|task| task.id.as_str()),
                &e.message,
            )
            .await;
            eprintln!("(jeeves) resumed turn failed: {}", e.message);
        }
    }
    true
}
