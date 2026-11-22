use crate::Server;
use jeeves_loop::LoopResult;
use jeeves_state::TaskRow;

/// What this turn decided about the task it was carrying. The *facts* decided it (acceptance,
/// budget, contradictions); this enum is just the verdict travelling to the one place that
/// writes it down.
#[derive(Debug, Clone)]
pub(crate) enum TaskDecision {
    /// Acceptance passed (or there was nothing objective to check) and nothing objected.
    Done,
    /// A machine may not settle this: verification failed, budget spent, side effect unlanded.
    /// The task stays **open** — a human can continue or cancel it.
    NeedsAttention(String),
    WaitingForJobs(Vec<String>),
    WaitingForInput(String),
    /// The loop's own verdict: not achievable, or the round cap was reached.
    Failed(String),
}

/// Checkpoint recovery and continuation observe the same round with identical operation keys.
pub(crate) fn observe_round(
    result: &LoopResult,
    turn_id: &str,
    cwd: Option<&std::path::Path>,
) -> jeeves_agent::observe::Observation {
    jeeves_agent::observe::observe(
        &result.messages,
        &format!("{turn_id}:{}", result.round_id),
        1,
        cwd,
    )
}

/// What one round's accounting needs. A struct because the fields would otherwise be seven
/// positional parameters that all mean different things.
pub(crate) struct RoundCtx<'a> {
    pub(crate) task: Option<&'a TaskRow>,
    pub(crate) run_id: Option<&'a str>,
    pub(crate) acceptance: &'a [jeeves_agent::verify::Acceptance],
    /// Where acceptance paths resolve (the task's own directory, else this turn's).
    pub(crate) verify_root: Option<std::path::PathBuf>,
    /// When this round started, for the wall-clock spend.
    pub(crate) started: std::time::Instant,
    pub(crate) wall_offset_ms: i64,
}

/// Record one round and measure it. Returns the verdicts (empty = nothing objective to check).
///
/// Cost: N `stat`s + one INSERT batch, no model call. When a condition fails this saves the
/// judge call the old flow would have spent being talked into "done".
pub(crate) async fn account_round(
    server: &Server,
    ctx: RoundCtx<'_>,
    observation: &jeeves_agent::observe::Observation,
    result: &LoopResult,
) -> Result<Vec<jeeves_agent::verify::CheckResult>, String> {
    let Some(task) = ctx.task else {
        return Ok(Vec::new());
    };

    let run_id = ctx.run_id.ok_or_else(|| {
        format!(
            "task {} has no run; accounting cannot be committed",
            task.id
        )
    })?;

    let recorded = jeeves_agent::observe::Recorder {
        steps: &server.steps,
        artifacts: &server.artifacts,
        events: &server.events,
        task_id: &task.id,
        run_id: Some(run_id),
    }
    .record(observation)
    .await;

    let wall_ms = ctx
        .wall_offset_ms
        .saturating_add(ctx.started.elapsed().as_millis().min(i64::MAX as u128) as i64);
    let ok = observation.steps.iter().filter(|step| step.ok).count() as i64;
    let account = jeeves_state::runs::RoundAccount {
        tokens_in: result.usage.input_tokens,
        tokens_out: result.usage.output_tokens,
        tools_ok: ok,
        tools_failed: observation.steps.len() as i64 - ok,
        wall_ms,
    };
    server
        .runs
        .account_round(run_id, &result.round_id, &account)
        .await
        .map_err(|e| format!("round accounting failed: {e}"))?;

    recorded.map_err(|error| {
        format!(
        "Task observation could not be recorded: {error}; review the completed work before retrying"
    )
    })?;

    let checks = if ctx.acceptance.is_empty() {
        Vec::new()
    } else {
        let commands = server.check_runner(
            ctx.verify_root.as_deref(),
            crate::task::worktree::owned(&server.settings().home, Some(task)),
        );
        jeeves_agent::verify::Verifier {
            run_id: run_id.to_string(),
            root: ctx.verify_root,
            steps: server.steps.clone(),
            task_id: task.id.clone(),
            receipts: jeeves_state::Receipts::new(server.pool.clone()),
            commands: Some(commands),
        }
        .run(ctx.acceptance)
        .await
    };

    if !checks.is_empty() {
        let views: Vec<_> = checks.iter().map(|check| check.view()).collect();
        let json = serde_json::to_string(&views)
            .map_err(|error| format!("Acceptance results could not be serialized: {error}"))?;
        let files: Vec<_> = checks
            .iter()
            .filter_map(|check| check.artifact.clone())
            .collect();
        jeeves_state::verification::Verifications::new(server.pool.clone())
            .record(
                &task.id,
                run_id,
                &result.round_id,
                &task.acceptance_json,
                &json,
                &files,
            )
            .await
            .map_err(|error| format!("Acceptance evidence could not be committed: {error}"))?;
        let detail = if jeeves_agent::verify::all_passed(&checks) {
            Some("All acceptance checks passed".to_string())
        } else {
            jeeves_agent::verify::failure_summary(&checks)
        };
        if let Err(e) = server
            .events
            .append(&task.id, Some(run_id), "verify", detail.as_deref())
            .await
        {
            eprintln!("(jeeves) acceptance event not recorded: {e}");
        }
    }
    Ok(checks)
}

/// Did this task already spend past one of its caps? One row read, a few comparisons.
///
/// Reaching the cap counts as over it: the next round would spend more, and the whole point of
/// a cap is to stop **before** the money is gone, not after.
pub(crate) fn budget_overrun(task: &TaskRow) -> Option<String> {
    budget_remaining(task).err()
}

/// Whether a valid, unspent task has an explicit cap. Any error stops further spending.
pub(crate) fn budget_remaining(task: &TaskRow) -> Result<bool, String> {
    let budget: jeeves_protocol::BudgetView =
        serde_json::from_str(&task.budget_json).map_err(|e| format!("Invalid task budget: {e}"))?;
    for (name, value) in [
        ("wallMs", budget.wall_ms),
        ("tokens", budget.tokens),
        ("tools", budget.tools),
    ] {
        if value.is_some_and(|value| value <= 0) {
            return Err(format!(
                "Invalid task budget {name}: the cap must be greater than zero"
            ));
        }
    }
    let mut over = Vec::new();
    if let Some(cap) = budget.wall_ms {
        if task.spent_wall_ms >= cap as i64 {
            over.push(format!(
                "elapsed {:.1}s (cap {:.1}s)",
                task.spent_wall_ms as f64 / 1000.0,
                cap as f64 / 1000.0
            ));
        }
    }
    if let Some(cap) = budget.tokens {
        if task.spent_tokens >= cap as i64 {
            over.push(format!("tokens {} (cap {})", task.spent_tokens, cap));
        }
    }
    if let Some(cap) = budget.tools {
        if task.spent_tools >= cap as i64 {
            over.push(format!("tool calls {} (cap {})", task.spent_tools, cap));
        }
    }
    if over.is_empty() {
        Ok(budget.wall_ms.is_some() || budget.tokens.is_some() || budget.tools.is_some())
    } else {
        Err(format!("Over budget: {}", over.join("; ")))
    }
}
