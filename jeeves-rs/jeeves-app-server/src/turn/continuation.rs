use super::{fail_turn, full_turn, unfinished_work, FullTurn, TurnFrame};
use crate::Server;
use jeeves_protocol::ErrorObject;
use jeeves_provider::Resolved;

/// A safety ceiling for budgeted task runs: the budget is the real limit, this only stops a
/// bug from looping forever.
const TASK_CONTINUE_CEILING: usize = 64;

/// This entry point drives fresh rounds. The final accounting pass persists their cumulative
/// usage once. Include the in-flight slice when deciding whether to buy the next one.
async fn slice_limit(
    server: &Server,
    id: &str,
    tokens: i64,
    tools: i64,
    wall_ms: i64,
    fixed_limit: usize,
) -> Result<usize, String> {
    let mut task = server
        .tasks
        .get(id)
        .await
        .map_err(|e| format!("Cannot read task budget: {e}"))?
        .ok_or_else(|| "Task no longer exists".to_string())?;
    task.spent_tokens = task.spent_tokens.saturating_add(tokens);
    task.spent_tools = task.spent_tools.saturating_add(tools);
    task.spent_wall_ms = task.spent_wall_ms.saturating_add(wall_ms);
    Ok(if super::budget_remaining(&task)? {
        TASK_CONTINUE_CEILING
    } else {
        fixed_limit
    })
}

pub(super) async fn continue_turn(
    server: &Server,
    resolved: &Resolved,
    mut full: FullTurn,
    frame: TurnFrame<'_>,
    run_id: Option<&str>,
    task: Option<&jeeves_state::TaskRow>,
    started: std::time::Instant,
) -> Result<(FullTurn, usize, Option<String>), ErrorObject> {
    let task_id = task.map(|row| row.id.as_str());
    let has_goal = task.is_some_and(|row| row.goal_condition.is_some());
    let settings = server.settings();
    let auto_continue_max = settings.auto_continue_max.max(0) as usize;

    let mut continues = 0usize;
    let mut earlier_calls: Vec<jeeves_loop::ToolOutcome> = Vec::new();
    let mut cap = settings.max_iterations.max(1);
    let unfinished: Option<String> = loop {
        let open_todos = if !has_goal {
            unfinished_work(server, frame.session_id, &full.result)
        } else {
            None
        };
        let cut_off = full.result.iterations >= cap;
        let reason = open_todos.clone().filter(|_| cut_off);
        let Some(reason) = reason else {
            break open_todos;
        };
        let limit = if let Some(id) = task_id {
            let usage = full.result.usage;
            let tokens = usage.input_tokens.saturating_add(usage.output_tokens);
            let tools = (earlier_calls.len() + full.result.tool_calls.len()) as i64;
            let wall_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
            match slice_limit(server, id, tokens, tools, wall_ms, auto_continue_max).await {
                Ok(limit) => limit,
                Err(stop) => {
                    full.result
                        .reply
                        .push_str(&format!("\n\n(Stopped: {stop}. Still open: {reason}.)"));
                    break Some(reason);
                }
            }
        } else {
            auto_continue_max
        };
        if continues >= limit {
            full.result.reply.push_str(&format!(
                "\n\n(One step short: {reason}. The auto-continue budget is spent — say \"carry on\" and I resume where it stopped.)"
            ));
            break Some(reason);
        }
        continues += 1;
        eprintln!("(jeeves) auto-continue {continues} of {limit}: {reason}");
        if let Some(id) = task_id {
            if let Err(error) = server
                .events
                .append(id, run_id, "auto_continue", Some(&reason))
                .await
            {
                eprintln!("(jeeves) auto-continue event not recorded: {error}");
            }
        }
        let nudge = format!(
            "{reason}\n\nCarry on: continue from where it stopped, do not restate what is done, do not re-plan."
        );
        cap = full.result.iterations + settings.turn_step_slice.max(1);
        let resume = jeeves_loop::Resume {
            round_id: full.result.round_id.clone(),
            messages: full.result.messages.clone(),
            completed_iterations: full.result.iterations,
            usage: full.result.usage,
            max_iterations: Some(cap),
        };
        let continued = full_turn(
            server,
            resolved,
            &nudge,
            Vec::new(),
            TurnFrame {
                session_id: frame.session_id,
                turn_id: frame.turn_id,
                sink: frame.sink,
                interrupt: frame.interrupt.clone(),
                worktree: frame.worktree.clone(),
                turn_cwd: frame.turn_cwd.clone(),
                route_reason: frame.route_reason.clone(),
                steering: frame.steering.clone(),
                resume: Some(resume),
                checkpoint: frame.checkpoint.clone(),
            },
            None,
        )
        .await;
        match continued {
            Ok(next) => {
                earlier_calls.append(&mut full.result.tool_calls);
                full = next;
            }
            Err(e) => {
                fail_turn(
                    server,
                    frame.sink,
                    frame.turn_id,
                    run_id,
                    task_id,
                    &e.message,
                )
                .await;
                return Err(e);
            }
        }
    };

    if !earlier_calls.is_empty() {
        earlier_calls.append(&mut full.result.tool_calls);
        full.result.tool_calls = earlier_calls;
    }

    Ok((full, continues, unfinished))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn server_with_task(budget: &str, spent_tokens: i64) -> Server {
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
            .tasks
            .add_spent("t1", 0, spent_tokens, 0)
            .await
            .unwrap();
        server
    }

    #[tokio::test]
    async fn a_task_without_a_budget_keeps_the_fixed_count() {
        let server = server_with_task("{}", 0).await;
        assert_eq!(slice_limit(&server, "t1", 0, 0, 0, 2).await.unwrap(), 2);
        assert!(slice_limit(&server, "missing", 0, 0, 0, 2).await.is_err());
    }

    #[tokio::test]
    async fn all_three_in_flight_counters_apply_to_slice_admission() {
        let server = server_with_task(r#"{"tokens":100,"tools":2,"wallMs":50}"#, 10).await;
        for (tokens, tools, wall, label) in [
            (90, 0, 0, "tokens"),
            (0, 2, 0, "tool"),
            (0, 0, 50, "elapsed"),
        ] {
            assert!(slice_limit(&server, "t1", tokens, tools, wall, 2)
                .await
                .unwrap_err()
                .contains(label));
        }
        assert_eq!(
            slice_limit(&server, "t1", 0, 0, 0, 2).await.unwrap(),
            TASK_CONTINUE_CEILING
        );
        let bad = server_with_task("broken", 0).await;
        assert!(slice_limit(&bad, "t1", 0, 0, 0, 2).await.is_err());
    }
}
