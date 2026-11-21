//! Long tasks: the goal loop's condition with an identity — and a pause button.
//!
//! A goal used to be a string in process memory. Here it becomes a row (`agent_tasks`): named,
//! addressable, suspendable, surviving restarts. Since 0017 the row also stands on its own:
//! it may belong to no session, it carries acceptance conditions, a budget, and what has been
//! spent. The goal loop is still the engine that drives rounds; this module is the ledger and
//! the human's switches.
//!
//! Discipline:
//! * **A task may exist without a session.** "Do this thing" does not require a conversation.
//! * **Every method here is human-only.** The model has no task tool; "the model assigning
//!   itself long work" is not forbidden — it does not exist.
//! * **Suspending keeps the checkpoint.** A suspension is not a stop: the turn's working
//!   memory stays `suspended` in state.db, and `task/resume` continues from it — finished
//!   tools are not re-run. `task/cancel` is the stop, and it abandons the checkpoint.
//! * **Nothing is silent.** Every transition sends `TaskUpdated` (the hub folds it into a ping)
//!   and the reason is stored, not dropped.

pub(crate) mod git_checkpoint;
pub(crate) mod integration;
mod workspace;
pub(crate) mod worktree;

use jeeves_protocol::{
    codes, AcceptanceView, AgentTaskView, ArtifactView, BudgetView, ErrorObject, RunView,
    ServerNotification, SpentView, TaskActionParams, TaskArtifactsResponse, TaskCreateParams,
    TaskEnvelope, TaskListResponse, TaskRunsResponse, TaskUpdatedNotification, TaskViewResponse,
};
use jeeves_state::{TaskRow, TaskStatus};

use crate::{dispatch::internal, narrow, EventSink, Server};
use jeeves_agent::verify::Acceptance;

fn stored<T: serde::de::DeserializeOwned>(field: &str, json: &str) -> Result<T, ErrorObject> {
    serde_json::from_str(json)
        .map_err(|e| internal(format!("Cannot read stored task {field}: {e}")))
}

/// The protocol view of a row — one modelling, shared by every method.
///
/// `async` because the run count is a second read: the card says "executed N times", and that
/// number has to come from the run ledger, not from a guess.
pub(crate) async fn view(server: &Server, row: &TaskRow) -> Result<AgentTaskView, ErrorObject> {
    let run_count = server
        .runs
        .count_by_task(&row.id)
        .await
        .map(narrow)
        .map_err(internal)?;
    let wt = worktree::of(&server.settings().home, &row.id, row.cwd.as_deref());
    let source = match jeeves_state::task_sources::get(&server.pool, &row.id).await {
        Ok(source) => source.map(|s| jeeves_protocol::InteractionSource {
            channel: s.channel,
            chat_id: s.chat_id,
            message_id: s.message_id,
            sender_id: s.sender_id,
        }),
        Err(error) => {
            eprintln!("(jeeves) task source unavailable: {error}");
            None
        }
    };
    let dispatch = match jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .get(&row.id)
        .await
    {
        Ok(dispatch) => dispatch,
        Err(error) => {
            eprintln!("(jeeves) task dispatch view unavailable: {error}");
            None
        }
    };
    let input_condition =
        match jeeves_state::external_waits::ExternalWaits::new(server.pool.clone())
            .get(&row.id)
            .await
        {
            Ok(condition) => condition.map(crate::task::inputs::view),
            Err(error) => {
                eprintln!("(jeeves) task input view unavailable: {error}");
                None
            }
        };
    let budget: BudgetView = stored("budget", &row.budget_json)?;
    Ok(AgentTaskView {
        parent_task_id: row.parent_task_id.clone(),
        input_condition,
        execution_session_id: dispatch.as_ref().map(|d| d.session_id.clone()),
        dispatch_state: dispatch.map(|d| d.state),
        id: row.id.clone(),
        session_id: row.session_id.clone(),
        title: row.title.clone(),
        goal_condition: row.goal_condition.clone(),
        status: row.status.as_str().to_string(),
        rounds_used: row.rounds_used,
        max_rounds: row.max_rounds,
        tier: row.tier.clone(),
        current_turn_id: row.current_turn_id.clone(),
        suspend_reason: row.suspend_reason.clone(),
        last_stop_reason: row.last_stop_reason.clone(),
        acceptance: stored("acceptance", &row.acceptance_json)?,
        checks: stored("checks", &row.checks_json)?,
        budget: (budget.wall_ms.is_some() || budget.tokens.is_some() || budget.tools.is_some())
            .then_some(budget),
        spent: SpentView {
            wall_ms: narrow(row.spent_wall_ms),
            tokens: narrow(row.spent_tokens),
            tools: narrow(row.spent_tools),
        },
        cwd: row.cwd.clone(),
        worktree_path: wt.as_ref().map(|w| w.0.clone()),
        branch: wt.map(|w| w.1),
        run_count,
        source,
        created_at: row.created_at.clone(),
        finished_at: row.finished_at.clone(),
    })
}

fn invalid(message: impl Into<String>) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

pub(crate) fn notify(sink: &EventSink, row: &TaskRow, reason: Option<&str>) {
    sink.notification(ServerNotification::TaskUpdated(TaskUpdatedNotification {
        task_id: row.id.clone(),
        session_id: row.session_id.clone(),
        status: row.status.as_str().to_string(),
        rounds_used: row.rounds_used,
        reason: reason.map(str::to_string),
    }));
}

/// Read back the row after a write so the caller answers with the authoritative state.
async fn reload(server: &Server, id: &str) -> Result<TaskRow, ErrorObject> {
    server
        .tasks
        .get(id)
        .await
        .map_err(|e| internal(format!("the task could not be read back: {e}")))?
        .ok_or_else(|| invalid("no such task"))
}

/// Normalise + validate the acceptance list. Refusing at creation is deliberate: a condition
/// nobody can check would sit on the card looking authoritative while never failing.
fn normalise_acceptance(views: &[AcceptanceView]) -> Result<String, ErrorObject> {
    let mut normalised = Vec::with_capacity(views.len());
    for (index, view) in views.iter().enumerate() {
        let acceptance = Acceptance::from_view(view)
            .map_err(|why| invalid(format!("acceptance check {} is invalid: {why}", index + 1)))?;
        let mut view = view.clone();
        if view
            .label
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .is_none()
        {
            view.label = Some(acceptance.label());
        }
        if view.min.is_none() {
            view.min = Some(1);
        }
        normalised.push(view);
    }
    serde_json::to_string(&normalised).map_err(|e| {
        invalid(format!(
            "the acceptance criteria could not be written as JSON: {e}"
        ))
    })
}

/// Same for the budget: a zero or negative cap is a typo, not an intention.
pub(crate) fn normalise_budget(budget: Option<&BudgetView>) -> Result<String, ErrorObject> {
    let Some(budget) = budget else {
        return Ok("{}".to_string());
    };
    for (name, value) in [
        ("wallMs", budget.wall_ms),
        ("tokens", budget.tokens),
        ("tools", budget.tools),
    ] {
        if let Some(value) = value {
            if value <= 0 {
                return Err(invalid(format!(
                    "budget {name} must be greater than 0 (got {value})"
                )));
            }
        }
    }
    serde_json::to_string(budget)
        .map_err(|e| invalid(format!("the budget could not be written as JSON: {e}")))
}

pub(crate) async fn create(
    server: &Server,
    params: TaskCreateParams,
) -> Result<TaskEnvelope, ErrorObject> {
    let title = params.title.trim();
    if title.is_empty() {
        return Err(invalid("a task needs a name"));
    }
    let session = params
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let max_rounds = params
        .max_rounds
        .unwrap_or_else(|| server.settings().goal_max_rounds)
        .max(1);
    let goal = params
        .goal_condition
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let tier = crate::route::clean_tier(params.tier.as_deref())?;
    let (edit_checks, final_checks) = split_checks(&params.code_checks)?;
    let mut acceptance = params.acceptance.clone();
    acceptance.extend(final_checks);
    let acceptance_json = normalise_acceptance(&acceptance)?;
    let budget_json = normalise_budget(params.budget.as_ref())?;
    let id = workspace::task_id();
    let repo = params
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    let parent = params.parent_task_id.as_deref();
    let parent_lock = parent.map(|id| server.session_write_lock(&format!("task-control:{id}")));
    let _parent_guard = match &parent_lock {
        Some(lock) => Some(lock.lock().await),
        None => None,
    };
    let parent_root = if let Some(parent) = parent {
        if repo.is_some() || params.cwd.is_some() {
            return Err(invalid(
                "parentTaskId chooses the repository; omit repo and cwd",
            ));
        }
        Some(integration::parent_root(server, parent).await?)
    } else {
        None
    };
    let repo = parent_root.as_deref().or(repo);
    let cwd_path = match (repo, crate::turn::resolve_turn_cwd(&params.cwd)?) {
        (Some(_), Some(_)) => return Err(invalid("give either a repo or a cwd, not both")),
        (Some(repo), None) => {
            Some(worktree::ensure(server.settings().home, id.clone(), repo.to_string()).await?)
        }
        (None, Some(path)) => Some(path),
        (None, None) => Some(workspace::allocate(server, &id, title).await?),
    };
    let cwd_text = cwd_path.as_ref().map(|path| path.display().to_string());
    if let Some(path) = &cwd_path {
        jeeves_config::workspaces::record_workspace(
            &server.settings().home,
            &path.display().to_string(),
        );
    }

    server
        .tasks
        .create_with_parent(
            jeeves_state::NewTask {
                id: &id,
                session_id: session,
                title,
                goal_condition: goal,
                acceptance_json: &acceptance_json,
                budget_json: &budget_json,
                cwd: cwd_text.as_deref(),
                max_rounds,
                tier: tier.as_deref(),
            },
            parent,
        )
        .await
        .map_err(|e| invalid(format!("the task was not created: {e}")))?;

    if !edit_checks.is_empty() {
        let json = serde_json::to_string(&edit_checks).map_err(|e| invalid(e.to_string()))?;
        jeeves_state::EditChecks::new(server.pool.clone())
            .set(&id, &json)
            .await
            .map_err(|e| invalid(format!("the task's edit checks were not saved: {e}")))?;
    }
    let row = reload(server, &id).await?;
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

/// `after: final` checks become `commandSucceeds` conditions; `after: edit` ones are returned to
/// be stored. Anything else is refused, with the check's name.
fn split_checks(
    checks: &[jeeves_protocol::CodeCheckSpec],
) -> Result<(Vec<jeeves_protocol::CodeCheckSpec>, Vec<AcceptanceView>), ErrorObject> {
    let (mut edit, mut last) = (Vec::new(), Vec::new());
    for check in checks {
        if check.argv.is_empty() || check.argv[0].trim().is_empty() {
            return Err(invalid(format!("check '{}' has no command", check.name)));
        }
        match check.after.as_str() {
            "edit" => edit.push(check.clone()),
            "final" => last.push(AcceptanceView {
                kind: "commandSucceeds".into(),
                label: Some(format!(
                    "check '{}': `{}` exits 0",
                    check.name,
                    check.argv.join(" ")
                )),
                argv: Some(check.argv.clone()),
                ..Default::default()
            }),
            other => {
                return Err(invalid(format!(
                    "check '{}': after must be 'edit' or 'final', not '{other}'",
                    check.name
                )))
            }
        }
    }
    Ok((edit, last))
}

/// Explicit human authorization for one durable initial execution.
pub(crate) async fn start(
    server: &Server,
    sink: &EventSink,
    params: jeeves_protocol::TaskStartParams,
) -> Result<TaskEnvelope, ErrorObject> {
    start_with_origin(server, sink, params, None).await
}

pub(crate) async fn start_with_origin(
    server: &Server,
    sink: &EventSink,
    params: jeeves_protocol::TaskStartParams,
    origin_message_id: Option<&str>,
) -> Result<TaskEnvelope, ErrorObject> {
    if server.resolved().is_none() {
        return Err(invalid("Configure a model before starting a task"));
    }
    let lock = server.session_write_lock(&format!("task-control:{}", params.task_id));
    let _guard = lock.lock().await;
    let row = reload(server, &params.task_id).await?;
    integration::require_integrated(server, &row.id).await?;
    jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .enqueue_with_origin(
            &row.id,
            &params.message,
            &serde_json::to_string(&params.attachments.unwrap_or_default())
                .map_err(|e| invalid(e.to_string()))?,
            origin_message_id,
        )
        .await
        .map_err(|e| invalid(format!("Task was not queued: {e}")))?;
    notify(sink, &row, Some("queued for independent execution"));
    crate::task::dispatch::kick(server.clone(), sink.clone());
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

pub(crate) async fn list(
    server: &Server,
    cursor: Option<&str>,
) -> Result<TaskListResponse, ErrorObject> {
    let mut rows = server.tasks.page(cursor, 101).await.map_err(internal)?;
    let next_cursor = if rows.len() > 100 {
        rows.pop();
        rows.last().map(|r| r.id.clone())
    } else {
        None
    };
    let mut tasks = Vec::with_capacity(rows.len());
    for row in &rows {
        tasks.push(view(server, row).await?);
    }
    Ok(TaskListResponse { tasks, next_cursor })
}

pub(crate) async fn view_of(
    server: &Server,
    session_id: &str,
) -> Result<TaskViewResponse, ErrorObject> {
    let task = match server.tasks.latest(session_id).await.map_err(internal)? {
        Some(row) => Some(view(server, &row).await?),
        None => None,
    };
    Ok(TaskViewResponse { task })
}

/// `task/get`: address one task by id. The only way in for a task with no session.
pub(crate) async fn get(server: &Server, task_id: &str) -> Result<TaskEnvelope, ErrorObject> {
    let row = reload(server, task_id).await?;
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

/// `task/events`: the timeline after a cursor, oldest first, one page at a time.
pub(crate) async fn events(
    server: &Server,
    p: jeeves_protocol::TaskEventsParams,
) -> Result<jeeves_protocol::TaskEventsResponse, ErrorObject> {
    let cursor = p.cursor.unwrap_or(0).max(0);
    let limit = p
        .limit
        .unwrap_or(100)
        .clamp(1, jeeves_state::events::MAX_SINCE);
    let task = p.task_id.as_deref();
    let read = |e: anyhow::Error| invalid(format!("task events could not be read: {e}"));
    let (rows, next) = server
        .events
        .since(i64::from(cursor), task, limit)
        .await
        .map_err(read)?;
    let latest = server.events.latest_cursor(task).await.map_err(read)?;
    let has_more = latest > next;
    Ok(jeeves_protocol::TaskEventsResponse {
        events: rows
            .into_iter()
            .map(|r| jeeves_protocol::TaskEventView {
                cursor: narrow(r.id),
                task_id: r.task_id,
                run_id: r.run_id,
                seq: r.seq,
                kind: r.kind,
                detail: r.detail,
                at: r.at,
            })
            .collect(),
        next_cursor: narrow(next),
        has_more,
        latest_cursor: narrow(latest),
    })
}

/// `task/runs`: every attempt of one task, newest first.
pub(crate) async fn runs(server: &Server, task_id: &str) -> Result<TaskRunsResponse, ErrorObject> {
    let rows = server
        .runs
        .list_by_task(task_id, 50)
        .await
        .map_err(internal)?;
    Ok(TaskRunsResponse {
        runs: rows
            .iter()
            .map(|row| RunView {
                id: row.id.clone(),
                status: row.status.clone(),
                rounds: row.rounds,
                tokens_in: narrow(row.tokens_in),
                tokens_out: narrow(row.tokens_out),
                tools_ok: narrow(row.tools_ok),
                tools_failed: narrow(row.tools_failed),
                stop_reason: row.stop_reason.clone(),
                started_at: row.started_at.clone(),
                ended_at: row.ended_at.clone(),
            })
            .collect(),
    })
}

/// `task/artifacts`: the files this task produced.
pub(crate) async fn artifacts(
    server: &Server,
    task_id: &str,
) -> Result<TaskArtifactsResponse, ErrorObject> {
    let rows = server.artifacts.list(task_id, 50).await.map_err(internal)?;
    Ok(TaskArtifactsResponse {
        artifacts: rows
            .iter()
            .map(|row| ArtifactView {
                path: row.path.clone(),
                bytes: narrow(row.bytes),
                at: row.at.clone(),
            })
            .collect(),
    })
}

/// Pause: the running turn is interrupted, but its checkpoint is **kept** (status `suspended`),
/// so resuming continues instead of replaying. The flag tells `finalize_turn` which of the two
/// exits this is — a human pause, not a stop.
pub(crate) async fn suspend(
    server: &Server,
    sink: &EventSink,
    params: TaskActionParams,
) -> Result<TaskEnvelope, ErrorObject> {
    let row = reload(server, &params.task_id).await?;
    let control = server.session_write_lock(&format!("task-control:{}", row.id));
    let _control = control.lock().await;
    let row = reload(server, &params.task_id).await?;
    let waits = jeeves_state::waits::JobWaits::new(server.pool.clone());

    let job_wait = waits
        .get(&row.id)
        .await
        .ok()
        .flatten()
        .filter(|wait| wait.state == "waiting");
    let inputs = jeeves_state::external_waits::ExternalWaits::new(server.pool.clone());
    let input_wait = inputs
        .waiting(&row.id)
        .await
        .map_err(|error| invalid(format!("Cannot read input wait: {error}")))?
        .filter(|wait| wait.state == "waiting");
    if row.status == TaskStatus::Suspended && job_wait.is_none() && input_wait.is_none() {
        return Ok(TaskEnvelope {
            task: view(server, &row).await?,
        });
    }
    if !matches!(
        row.status,
        TaskStatus::Running
            | TaskStatus::Pending
            | TaskStatus::NeedsAttention
            | TaskStatus::Suspended
    ) {
        return Err(invalid(format!(
            "the task has already ended ({}), so there is nothing to pause",
            row.status
        )));
    }
    let reason = params
        .reason
        .as_deref()
        .unwrap_or("paused by the user")
        .to_string();
    server.browser.stop_task(&row.id, false).await;
    if let Some(turn_id) = &row.current_turn_id {
        if !server.suspend_turn(turn_id) {
            eprintln!("(jeeves) task {turn_id} is no longer running; marking it suspended");
        }
    }
    server
        .tasks
        .suspend(&row.id, Some(&reason))
        .await
        .map_err(|e| invalid(format!("the pause was not recorded: {e}")))?;
    let row = reload(server, &row.id).await?;
    notify(sink, &row, Some(&reason));
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

/// Continue: a suspended (or attention-needing) task goes back to running. If its turn left a
/// `suspended` checkpoint, that turn continues from where it stopped; otherwise the task waits
/// for the next human message to drive it.
pub(crate) async fn resume(
    server: &Server,
    sink: &EventSink,
    params: TaskActionParams,
) -> Result<TaskEnvelope, ErrorObject> {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let row = reload(server, &params.task_id).await?;
        if !matches!(
            row.status,
            TaskStatus::Suspended | TaskStatus::NeedsAttention
        ) {
            break;
        }
        let checkpoint = crate::task::recovery::checkpoint(server, &row).await?;
        let turn_id = checkpoint
            .as_ref()
            .map(|cp| cp.turn_id.as_str())
            .or(row.current_turn_id.as_deref());
        let active = turn_id.is_some_and(|id| {
            server
                .active_turns(None)
                .iter()
                .any(|(active, _, _)| active == id)
        });
        if !active {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(invalid(
                "Previous execution is still stopping; continue again after it stops",
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    resume_ready(server, sink, params).await
}

async fn resume_ready(
    server: &Server,
    sink: &EventSink,
    params: TaskActionParams,
) -> Result<TaskEnvelope, ErrorObject> {
    let row = reload(server, &params.task_id).await?;
    let control = server.session_write_lock(&format!("task-control:{}", row.id));
    let _control = control.lock().await;
    let row = reload(server, &params.task_id).await?;
    let waits = jeeves_state::waits::JobWaits::new(server.pool.clone());
    let pauses = jeeves_state::human_pauses::HumanPauses::new(server.pool.clone());
    let human_paused = pauses
        .is_paused(&row.id)
        .await
        .map_err(|error| invalid(format!("Cannot verify human pause: {error}")))?;

    if !human_paused
        && row.status == TaskStatus::Suspended
        && row.suspend_reason.as_deref() == Some(jeeves_state::waits::JOB_WAIT_REASON)
    {
        if let Ok(Some(wait)) = waits.get(&row.id).await {
            if wait.state == "waiting" {
                crate::task::waits::arm(server.clone(), sink.clone(), wait);
                return Ok(TaskEnvelope {
                    task: view(server, &row).await?,
                });
            }
        }
    }
    if !human_paused
        && row.status == TaskStatus::Suspended
        && row.suspend_reason.as_deref() == Some(jeeves_state::external_waits::INPUT_WAIT_REASON)
    {
        let wait = jeeves_state::external_waits::ExternalWaits::new(server.pool.clone())
            .waiting(&row.id)
            .await
            .map_err(|error| invalid(format!("Cannot read input wait: {error}")))?
            .filter(|wait| wait.state == "waiting")
            .ok_or_else(|| invalid("Input wait is unavailable; reconcile before continuing"))?;
        crate::input_wake::arm(server.clone(), sink.clone(), wait);
        return Ok(TaskEnvelope {
            task: view(server, &row).await?,
        });
    }
    if row.status == TaskStatus::Running {
        return Ok(TaskEnvelope {
            task: view(server, &row).await?,
        });
    }
    if !matches!(
        row.status,
        TaskStatus::Suspended | TaskStatus::NeedsAttention
    ) {
        return Err(invalid(format!(
            "only a suspended task, or one waiting for your call, can resume (currently {})",
            row.status
        )));
    }
    let unstarted = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
        .resume_unstarted(&row.id)
        .await
        .map_err(|e| invalid(format!("Cannot resume queued task: {e}")))?;
    if unstarted {
        let row = reload(server, &row.id).await?;
        notify(sink, &row, Some("queued task resumed by the user"));
        crate::task::dispatch::kick(server.clone(), sink.clone());
        return Ok(TaskEnvelope {
            task: view(server, &row).await?,
        });
    }
    let mut checkpoint = crate::task::recovery::checkpoint(server, &row).await?;
    if let Some(checkpoint) = &mut checkpoint {
        crate::task::recovery::require_known(checkpoint)?;
        crate::task::inputs::prepare_manual_resume(server, checkpoint)
            .await
            .map_err(|error| invalid(format!("Cannot prepare continuation: {error}")))?;
        pauses
            .resume_checkpoint(&row.id, &checkpoint.turn_id, &checkpoint.messages_json)
            .await
            .map_err(|error| invalid(format!("Cannot commit human continuation: {error}")))?;
    } else {
        if jeeves_state::dispatch::TaskDispatch::new(server.pool.clone())
            .get(&row.id)
            .await
            .map_err(|e| invalid(format!("Cannot read task dispatch: {e}")))?
            .is_some()
        {
            return Err(invalid("Task has no safe resumable checkpoint; reconcile the execution before starting another attempt"));
        }
        server
            .tasks
            .resume(&row.id)
            .await
            .map_err(|e| invalid(format!("The resume was not recorded: {e}")))?;
    }
    let row = reload(server, &row.id).await?;
    notify(sink, &row, Some("resumed by the user"));
    crate::task::dispatch::kick(server.clone(), sink.clone());
    if let Some(checkpoint) = checkpoint {
        crate::resume::resume_authorized(server, sink, checkpoint);
    }
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

/// Stop: interrupt whatever is running and close the row as `cancelled`. The checkpoint goes
/// `abandoned` — a cancelled task must never auto-continue on the next boot.
pub(crate) async fn cancel(
    server: &Server,
    sink: &EventSink,
    params: TaskActionParams,
) -> Result<TaskEnvelope, ErrorObject> {
    let row = reload(server, &params.task_id).await?;
    let control = server.session_write_lock(&format!("task-control:{}", row.id));
    let _control = control.lock().await;
    let row = reload(server, &params.task_id).await?;
    let waits = jeeves_state::waits::JobWaits::new(server.pool.clone());

    if row.status.is_terminal() {
        return Ok(TaskEnvelope {
            task: view(server, &row).await?,
        });
    }
    let reason = params.reason.as_deref().unwrap_or("cancelled by the user");
    server.browser.stop_task(&row.id, true).await;
    if let Some(turn_id) = &row.current_turn_id {
        server.interrupt_turn(turn_id);
        if let Err(e) = server.checkpoints.finish(turn_id, "abandoned").await {
            eprintln!("(jeeves) the checkpoint was not marked when cancelling (the cancellation itself is unaffected): {e}");
        }
    }
    server
        .tasks
        .finish(&row.id, TaskStatus::Cancelled)
        .await
        .map_err(|e| invalid(format!("the cancellation was not recorded: {e}")))?;
    if let Err(e) = waits.cancel(&row.id).await {
        eprintln!(
            "(jeeves) cancelled task {} still has a job wait to clean up: {e}",
            row.id
        );
    }
    if let Err(error) = jeeves_state::external_waits::ExternalWaits::new(server.pool.clone())
        .cancel(&row.id)
        .await
    {
        eprintln!("(jeeves) cancelled task input wait could not be closed: {error}");
    }

    if let (Some(host), Ok(jobs)) = (
        &server.jobs,
        jeeves_state::Jobs::new(server.pool.clone())
            .for_task(&row.id)
            .await,
    ) {
        for job in jobs.iter().filter(|job| job.status == "running") {
            if let Err(e) = host.kill(&job.owner, &job.id).await {
                eprintln!("(jeeves) task job {} could not be stopped: {e}", job.id);
            }
        }
    }
    if let Ok(runs) = server.runs.list_by_task(&row.id, 50).await {
        for run in runs
            .iter()
            .filter(|run| matches!(run.status.as_str(), "running" | "waiting" | "suspended"))
        {
            let _ = server
                .runs
                .close(&run.id, jeeves_state::RunStatus::Cancelled, Some(reason))
                .await;
        }
    }
    let row = reload(server, &row.id).await?;
    notify(sink, &row, Some(reason));
    Ok(TaskEnvelope {
        task: view(server, &row).await?,
    })
}

pub(crate) mod contract;
pub(crate) mod dispatch;
pub(crate) mod handoff;
pub(crate) mod inputs;
pub(crate) mod operations;
pub(crate) mod recovery;
pub(crate) mod waits;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod dispatch_tests;

#[cfg(test)]
mod operations_tests;

#[cfg(test)]
mod waits_tests;

#[cfg(test)]
mod recovery_tests;
