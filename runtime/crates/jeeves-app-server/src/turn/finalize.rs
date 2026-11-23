use super::{
    fold_tool_activity, settle_task, strip_echoed_footer, HistoryStats, Settlement, TaskDecision,
};
use crate::{narrow, trace, EventSink, Server};
use chrono::Local;
use jeeves_loop::LoopResult;
use jeeves_protocol::{
    codes, ConsolidationCompletedNotification, ErrorObject, ServerNotification, TokenUsage,
    ToolCallRecord, ToolStatus, TurnCompletedNotification, TurnMeta,
};
use jeeves_provider::Resolved;

/// Everything one finished turn leaves behind — shared by the fresh path and the resumed one,
/// so "a resumed turn gets exactly the same accounting" holds by construction, not by copy-paste.
pub(crate) struct FinishedTurn<'a> {
    /// What history/compaction looked like — the trace writes the before/after in turns.
    pub(crate) history: HistoryStats,
    pub(crate) session_id: String,
    pub(crate) turn_id: String,
    pub(crate) user_message: &'a str,
    pub(crate) resolved: &'a Resolved,
    pub(crate) result: LoopResult,
    pub(crate) route_reason: Option<String>,
    pub(crate) retries: i32,
    pub(crate) goal_status: Option<String>,
    pub(crate) goal_rounds: i32,
    pub(crate) started: std::time::Instant,
    /// The run (task attempt) this turn is; `None` for a task-less turn.
    pub(crate) run_id: Option<String>,
    /// The verdict of this turn's rounds; `None` = nobody decided (a resumed turn).
    pub(crate) decision: Option<TaskDecision>,
}

/// The shared tail of every turn: notifications, the checkpoint ledger's closing entry, the
/// trace, the usage ledger, the write section (history append → consolidation) and
/// the `TurnCompleted` announcement. The reply goes to the response by `run_turn` itself — only
/// it knows the order, and a resumed turn has no response to send at all.
pub(crate) async fn finalize_turn(
    server: &Server,
    sink: &EventSink,
    mut t: FinishedTurn<'_>,
) -> Result<(), ErrorObject> {
    let settings = server.settings();
    t.result.reply = strip_echoed_footer(&t.result.reply).to_string();
    let Settlement {
        carried_task,
        waiting_for_jobs,
        input_wait_committed,
    } = settle_task(server, sink, &mut t).await?;
    let FinishedTurn {
        session_id,
        turn_id,
        user_message,
        resolved,
        result,
        route_reason,
        retries,
        goal_status,
        goal_rounds,
        history,
        started,
        run_id,
        decision: _,
    } = t;

    crate::completion_delivery::flush(server).await;

    let spill_dir = settings.home.join("spill");
    let mut reply = fold_tool_activity(&result.reply, &result.tool_calls, Some(&spill_dir));
    if result.stop_reason == jeeves_provider::StopReason::MaxTokens && !result.interrupted {
        reply.push_str("\n\n(The output limit was reached; this response is incomplete.)");
    }
    if waiting_for_jobs {
        reply.push_str("\n\n(Background jobs are still running; task completion is pending.)");
    }

    if input_wait_committed {
        reply.push_str("\n\n(Waiting for the declared input; task completion is pending.)");
    }

    let meta = TurnMeta {
        iterations: result.iterations,
        latency_ms: started.elapsed().as_millis() as i32,
        tools: result
            .tool_calls
            .iter()
            .map(|c| ToolCallRecord {
                tool: c.name.clone(),
                status: if c.ok() {
                    ToolStatus::Ok
                } else {
                    ToolStatus::Error
                },
                duration_ms: Some(narrow(c.duration_ms)),
            })
            .collect(),
        model: resolved.model.clone(),
        provider: resolved.provider_id.clone(),
        usage: Some(TokenUsage {
            input_tokens: narrow(result.usage.input_tokens),
            output_tokens: narrow(result.usage.output_tokens),
        }),
        interrupted: result.interrupted,
        guard_hits: result.guard.hits,
        guard_note: result.guard.note.clone(),
        retries,
        goal_status: goal_status.clone(),
        goal_rounds,
        route_reason: route_reason.clone(),
    };

    let meta_json = serde_json::to_string(&meta).ok();

    trace::record_turn(
        &settings.home,
        &serde_json::json!({
            "ts": Local::now().to_rfc3339(),
            "turnId": turn_id,
            "sessionId": session_id,
            "userMessage": user_message,
            "reply": reply,
            "meta": meta,
            "tools": result
                .tool_calls
                .iter()
                .enumerate()
                .map(|(index, call)| serde_json::json!({
                    "tool": call.name,
                    "callIndex": index,
                    "durationMs": call.duration_ms,
                    "status": if call.ok() { "ok" } else { "error" },
                }))
                .collect::<Vec<_>>(),
            "retries": retries,
            "tokens": {
                "input": result.usage.input_tokens,
                "output": result.usage.output_tokens,
                "estimatedInput": result.estimated_input_tokens,
                "observedInput": result.observed_input_tokens,
                "turnBudget": settings.turn_token_budget,
            },
            "history": {
                "turnsTotal": history.turns_total,
                "turnsKept": history.turns_kept,
                "compacted": history.compacted,
            },
            "steps": result.iterations,
            "stopDetail": result.stop_detail,
        }),
    );
    let session_write_lock = server.session_write_lock(&session_id);
    let write_guard = session_write_lock.lock().await;

    let steers = server
        .steering_of(&turn_id)
        .map(|steering| {
            steering
                .lock()
                .expect("the steering lock cannot be poisoned")
                .all()
        })
        .unwrap_or_default();
    let browser_task = if let Some(task) = &carried_task {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM browser_sessions WHERE task_id=?)",
        )
        .bind(&task.id)
        .fetch_one(&server.pool)
        .await
        .unwrap_or(true)
    } else {
        false
    };
    server
        .chat
        .append_turn_for(
            user_message,
            &steers,
            &reply,
            &session_id,
            if browser_task {
                "browser-task"
            } else {
                "app-server"
            },
            meta_json.as_deref(),
            carried_task.as_ref().map(|task| task.id.as_str()),
        )
        .await
        .map_err(|e| ErrorObject {
            code: codes::INTERNAL_ERROR,
            message: format!("Could not write the conversation log: {e}"),
            data: None,
        })?;

    let ledger_sink = crate::ledger_writer::LedgerSink::new(
        &settings.home,
        settings.ledger_inline_max.max(0) as usize,
    );
    let ledger_entries = crate::ledger_writer::entries_for_turn(
        &ledger_sink,
        &session_id,
        &turn_id,
        user_message,
        &result.messages,
        &reply,
    );
    if let Err(e) = server
        .ledger
        .append(&session_id, &turn_id, &ledger_entries, user_message)
        .await
    {
        eprintln!(
            "(jeeves) ledger write failed (the conversation is stored; this turn is unaffected): {e}"
        );
    }

    if let Err(e) = server
        .chat
        .observe_context(
            &session_id,
            result.observed_input_tokens as i64,
            result.estimated_input_tokens as i64,
        )
        .await
    {
        eprintln!("(jeeves) observed context size not recorded (this turn is unaffected): {e}");
    }

    if waiting_for_jobs {
        if let Some(task) = &carried_task {
            if let Ok(Some(wait)) = jeeves_state::waits::JobWaits::new(server.pool.clone())
                .get(&task.id)
                .await
            {
                if wait.state == "waiting" {
                    crate::task::waits::arm(server.clone(), sink.clone(), wait);
                }
            }
        }
    }

    if input_wait_committed {
        if let Some(task) = &carried_task {
            if let Ok(Some(wait)) =
                jeeves_state::external_waits::ExternalWaits::new(server.pool.clone())
                    .waiting(&task.id)
                    .await
            {
                if wait.state == "waiting" {
                    crate::input_wake::arm(server.clone(), sink.clone(), wait);
                }
            }
        }
    }

    if let Some(worker) = server.memory_worker.as_ref() {
        worker.wake();
        let resolved = super::meter::metered(
            &server.pool,
            resolved,
            &session_id,
            &turn_id,
            run_id.as_deref(),
            "summary",
        );
        let (server, session_id) = (server.clone(), session_id.clone());
        tokio::spawn(
            async move { super::history::precompact(&server, &resolved, &session_id).await },
        );
    } else {
        let resolved = super::meter::metered(
            &server.pool,
            resolved,
            "workspace-memory",
            &crate::turn::new_turn_id(),
            None,
            "memory",
        );
        let new_facts = jeeves_memory::consolidation::consolidate_if_due(
            &server.chat,
            &server.facts,
            &server.episodes,
            resolved.client.as_ref(),
            &resolved.small_model,
            settings.consolidate_every,
        )
        .await
        .unwrap_or(0);
        if new_facts > 0 {
            sink.notification(ServerNotification::ConsolidationCompleted(
                ConsolidationCompletedNotification {
                    new_facts: new_facts as i32,
                },
            ));
        }
    }
    drop(write_guard);

    sink.notification(ServerNotification::TurnCompleted(Box::new(
        TurnCompletedNotification {
            turn_id,
            reply: result.reply.clone(),
            iterations: result.iterations,
            usage: Some(TokenUsage {
                input_tokens: narrow(result.usage.input_tokens),
                output_tokens: narrow(result.usage.output_tokens),
            }),
            meta,
        },
    )));
    Ok(())
}
