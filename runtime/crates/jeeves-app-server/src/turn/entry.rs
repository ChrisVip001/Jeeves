use chrono::Local;
use jeeves_protocol::{
    codes, ConsolidationCompletedNotification, ErrorObject, JsonRpcMessage, JsonRpcResponse,
    RequestId, ServerNotification, TurnStartParams, TurnStartResponse, TurnStartedNotification,
    JSONRPC_VERSION,
};
use jeeves_provider::Resolved;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use super::{
    checkpoint_outlet, continue_turn, driver, fail_turn, finalize_turn, full_turn, load_plan,
    new_turn_id, open_run, resolve_attachments, resolve_turn_cwd, retry_outlet, session_cwd,
    FinishedTurn, FullTurn, TurnFrame, TurnGuard,
};
use crate::{route, EventSink, Server};

pub async fn run_turn(
    server: &Server,
    params: TurnStartParams,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    run_turn_inner(server, params, id, sink, None, None, None).await
}

pub(crate) async fn run_turn_inner(
    server: &Server,
    params: TurnStartParams,
    id: RequestId,
    sink: &EventSink,
    dispatch: Option<jeeves_state::dispatch::DispatchRow>,
    interaction_tool: Option<&jeeves_tools::Tool>,
    interaction: Option<(&str, bool)>,
) -> Result<(), ErrorObject> {
    let started = std::time::Instant::now();
    let channel = interaction.is_some_and(|(_, channel)| channel);

    let session_id = params
        .session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());

    if let Some(worker) = server.memory_worker.as_ref() {
        let pending = worker.take_pending();
        if pending > 0 {
            sink.notification(ServerNotification::ConsolidationCompleted(
                ConsolidationCompletedNotification { new_facts: pending },
            ));
        }
    }

    let task = if channel {
        Ok(None)
    } else if let Some(dispatch) = &dispatch {
        server.tasks.get(&dispatch.task_id).await
    } else {
        server.tasks.active(&session_id).await
    }
    .map_err(|e| ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: format!("Cannot read task state: {e}"),
        data: None,
    })?;
    if dispatch.is_some() && task.is_none() {
        return Err(ErrorObject {
            code: codes::INVALID_PARAMS,
            message: "Dispatched task no longer exists".into(),
            data: None,
        });
    }
    let task_root = resolve_turn_cwd(&task.as_ref().and_then(|row| row.cwd.clone()))?;
    let worktree = crate::task::worktree::owned(&server.settings().home, task.as_ref());
    let requested = resolve_turn_cwd(&params.cwd)?;
    if let Some(root) = &worktree {
        jeeves_tools::roots::check_worktree(root, requested.as_deref().unwrap_or(root)).map_err(
            |message| ErrorObject {
                code: codes::INVALID_PARAMS,
                message,
                data: None,
            },
        )?;
    }
    let tier = route::clean_tier(params.tier.as_deref())?
        .or_else(|| task.as_ref().and_then(|task| task.tier.clone()));

    let resolved: Resolved = server.resolved().ok_or_else(|| ErrorObject {
        code: codes::PROVIDER_ERROR,
message: "The model is not configured yet: no API key. Open Settings → Models and add one (the key goes to the system keychain), or in a terminal: /model add <name> <provider> --env VAR."
            .to_string(),
        data: None,
    })?;

    let (resolved, turn_route) = server
        .resolved_for_turn(resolved, &params.message, tier.as_deref())
        .await;

    let user_images = resolve_attachments(server, &params, &resolved.provider_id, resolved.vision)?;

    let task_lock = task
        .as_ref()
        .map(|task| server.session_write_lock(&format!("task-control:{}", task.id)));
    let task_guard = match &task_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    if let Some(task) = &task {
        crate::task::integration::require_integrated(server, &task.id).await?;
        let current = server.tasks.get(&task.id).await.map_err(|e| ErrorObject {
            code: codes::INTERNAL_ERROR,
            message: format!("Cannot recheck task admission: {e}"),
            data: None,
        })?;
        let dispatched = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
            .get(&task.id)
            .await
            .map_err(|e| ErrorObject {
                code: codes::INTERNAL_ERROR,
                message: format!("Cannot read task dispatch: {e}"),
                data: None,
            })?;
        if !current.as_ref().is_some_and(|t| t.status.is_open())
            || (dispatch.is_none() && dispatched.is_some())
            || (dispatch.is_some()
                && !current
                    .as_ref()
                    .is_some_and(|t| t.status == jeeves_state::TaskStatus::Pending))
        {
            return Err(ErrorObject {
                code: codes::SESSION_BUSY,
                message: "Task admission changed; inspect its current state before continuing"
                    .into(),
                data: None,
            });
        }
    }

    if let Some(task) = &task {
        if let Some(checkpoint) = crate::task::recovery::checkpoint(server, task).await? {
            crate::task::recovery::require_known(&checkpoint)?;
        }
    }
    let turn_id = dispatch
        .as_ref()
        .map(|row| row.turn_id.clone())
        .or_else(|| {
            interaction.map(|(key, channel)| jeeves_protocol::interaction_turn_id(key, channel))
        })
        .unwrap_or_else(new_turn_id);

    if let Err(why) = server.turn_capacity() {
        eprintln!("(jeeves) turn refused: {why}");
        return Err(ErrorObject {
            code: codes::SESSION_BUSY,
            message: why,
            data: None,
        });
    }
    if let Err(why) = server.home_capacity().await {
        eprintln!("(jeeves) turn refused: {why}");
        return Err(ErrorObject {
            code: codes::INTERNAL_ERROR,
            message: why,
            data: None,
        });
    }
    let (interrupt, steering) =
        server
            .begin_turn(&turn_id, &session_id)
            .map_err(|holder| ErrorObject {
                code: codes::SESSION_BUSY,
                message: format!(
"A turn is already running in this session ({holder}) — wait for it to finish, or add a line with turn/steer; it is picked up on the turn's next round."
                ),
                data: None,
            })?;
    let _guard = TurnGuard {
        server,
        turn_id: turn_id.clone(),
        session_id: session_id.clone(),
    };

    let acceptance = match &task {
        Some(task) => crate::task::contract::admit(server, sink, task).await?,
        None => Vec::new(),
    };
    if let Some(task) = &task {
        match server.tasks.set_running(&task.id, &turn_id).await {
            Ok(()) => {
                if let Ok(Some(row)) = server.tasks.get(&task.id).await {
                    crate::task::notify(sink, &row, None);
                }
            }
            Err(e) => {
                return Err(ErrorObject {
                    code: codes::INTERNAL_ERROR,
                    message: format!(
                        "Task heartbeat could not be recorded; execution stopped: {e}"
                    ),
                    data: None,
                })
            }
        }
    }
    drop(task_guard);
    let task_id = task.as_ref().map(|task| task.id.clone());

    let plan = load_plan(server, &resolved, task.as_ref(), &acceptance).await;

    let run_id = open_run(server, task.as_ref(), &turn_id, &session_id).await;
    if task.is_some() && run_id.is_none() {
        return Err(ErrorObject {
            code: codes::INTERNAL_ERROR,
            message: "Task run could not be recorded; execution stopped".into(),
            data: None,
        });
    }
    let round_no = 1i32;
    let max_rounds = task
        .as_ref()
        .map(|row| row.max_rounds.max(1))
        .unwrap_or_else(|| {
            server
                .settings
                .read()
                .expect("the settings lock cannot be poisoned")
                .goal_max_rounds
                .max(1)
        });
    let checkpoint = checkpoint_outlet(
        server,
        sink,
        &turn_id,
        &session_id,
        &params.message,
        run_id.as_deref(),
        round_no,
    );

    let mut turn_cwd = requested.or(task_root);
    if !channel && turn_cwd.is_none() {
        turn_cwd = session_cwd(server, &session_id).await;
    }

    if let Some(path) = &turn_cwd {
        jeeves_config::workspaces::record_workspace(
            &server.settings().home,
            &path.display().to_string(),
        );
    }

    sink.notification(ServerNotification::TurnStarted(TurnStartedNotification {
        turn_id: turn_id.clone(),
        session_id: session_id.clone(),
        user_message: params.message.clone(),
        ts: Local::now().to_rfc3339(),
    }));

    let mut message = params.message.clone();
    if let Some(current) = &plan {
        message = format!("{message}\n\n{}", current.brief());
    }

    let retries = Arc::new(AtomicI32::new(0));
    let retry_sink = retry_outlet(sink, &turn_id, retries.clone());

    let frame = TurnFrame {
        interaction_tool,
        session_id: &session_id,
        turn_id: &turn_id,
        sink,
        interrupt: Some(interrupt),
        worktree,
        turn_cwd,
        steering: (!channel).then_some(steering),
        resume: None,
        checkpoint: Some(checkpoint),
    };
    let dispatched: Result<_, jeeves_protocol::ErrorObject> =
        jeeves_provider::retry::with_note_sink(retry_sink, async {
            full_turn(
                server,
                &resolved,
                &message,
                user_images.clone(),
                frame.clone(),
                None,
            )
            .await
        })
        .await;

    let full = match dispatched {
        Ok(full) => full,
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
    };

    let (full, continues, unfinished) = continue_turn(
        server,
        &resolved,
        full,
        frame.clone(),
        run_id.as_deref(),
        task.as_ref(),
        started,
    )
    .await?;

    let driver::DrivenTask {
        full,
        decision,
        goal_status,
        round_no,
    } = driver::drive_task(
        driver::DriveTask {
            server,
            resolved: &resolved,
            task: task.clone(),
            assignment: &params.message,
            run_id: run_id.clone(),
            frame,
            plan,
            round_no,
            max_rounds,
            started,
            wall_offset_ms: 0,
        },
        full,
    )
    .await?;

    let FullTurn {
        result, history, ..
    } = full;

    finalize_turn(
        server,
        sink,
        FinishedTurn {
            session_id,
            turn_id: turn_id.clone(),
            user_message: &params.message,
            resolved: &resolved,
            result,
            route_reason: turn_route,
            retries: retries.load(Ordering::Relaxed),
            goal_status,
            goal_rounds: round_no,
            history,
            started,
            run_id,
            decision: Some(decision),
            continues,
            unfinished,
        },
    )
    .await?;

    sink.response(JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id,
        result: serde_json::to_value(TurnStartResponse { turn_id }).expect("serialises"),
    }));
    Ok(())
}
