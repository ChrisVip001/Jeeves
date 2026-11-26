use std::sync::Arc;

use super::{
    FORCED_RETRY_TURNS, HistoryStats, Loaded, PRELOOP_TIMEOUT, PromptParts, SUMMARY_RESERVE_TOKENS,
    TurnFrame, budget_tokens, build_system, capability_note, interruptible, is_context_overflow,
    load_history, load_soul, retrieve_memory, thin_soul_for,
};
use crate::Server;
use jeeves_loop::{LoopEvent, LoopResult};
use jeeves_protocol::{
    ErrorObject, ServerNotification, TextDeltaNotification, ToolCompletedNotification,
    ToolDeltaNotification, ToolStartedNotification, ToolStatus, codes,
};
use jeeves_provider::{Resolved, TextSink};

/// The output of one full loop.
pub(crate) struct FullTurn {
    pub(crate) result: LoopResult,
    /// What history/compaction looked like for this turn — straight into the trace.
    pub(crate) history: HistoryStats,
}

pub(crate) async fn full_turn(
    server: &Server,
    resolved: &Resolved,
    message: &str,
    user_images: Vec<jeeves_provider::ContentBlock>,
    frame: TurnFrame<'_>,
    inner: Option<jeeves_loop::Observer>,
) -> Result<FullTurn, ErrorObject> {
    let TurnFrame {
        session_id,
        turn_id,
        sink,
        interrupt,
        turn_cwd,
        resume,
        checkpoint,
        steering,
        ..
    } = frame.clone();
    let settings = server.settings();
    let run = server
        .runs
        .for_turn(turn_id)
        .await
        .map_err(|error| ErrorObject {
            code: codes::INTERNAL_ERROR,
            message: format!("Cannot resolve task execution identity: {error}"),
            data: None,
        })?;
    let owner_run = run.as_ref().map(|r| r.id.as_str());
    let execution = with_fallback(server, resolved, |model| {
        super::meter::metered(
            &server.pool,
            model,
            session_id,
            turn_id,
            owner_run,
            "execution",
        )
    });
    let summary_model = super::meter::metered(
        &server.pool,
        resolved,
        session_id,
        turn_id,
        owner_run,
        "summary",
    );
    let scoped_task = if let Some(run) = &run {
        jeeves_state::task_sources::get(&server.pool, &run.task_id)
            .await
            .map_err(|error| ErrorObject {
                code: codes::INTERNAL_ERROR,
                message: format!("Cannot resolve task origin: {error}"),
                data: None,
            })?
            .is_some()
    } else {
        false
    };
    let channel_chat = turn_id.starts_with(jeeves_protocol::CHANNEL_TURN_PREFIX);
    let shared = channel_chat || scoped_task;
    let chat_operations = if channel_chat {
        crate::turn::operations::ChatContext {
            unresolved: false,
            prompt: String::new(),
        }
    } else {
        crate::turn::operations::context(server, session_id).await?
    };
    let task_handoff = crate::task::handoff::load(server, turn_id, session_id).await?;

    let memory_context = if shared {
        String::new()
    } else {
        retrieve_memory(server, message, &settings, &frame).await
    };

    let mut skill_loader = jeeves_memory::skills::SkillLoader::new(if shared {
        Vec::new()
    } else {
        jeeves_memory::skills::SkillLoader::dirs_for(&settings.home)
    });
    let skill_hits = skill_loader.hits_capped(message, settings.skill_body_tokens.max(1) as usize);
    let skills = skill_hits.section;
    let skill_index = skill_loader.index_text(settings.skill_index_tokens.max(1) as usize);
    let prompt_message = if skill_hits.hints.is_empty() {
        skill_hits.message.clone()
    } else {
        format!("{}\n\n{}", skill_hits.hints.join("\n"), skill_hits.message)
    };
    let prompt_message = if resume.is_some() {
        prompt_message
    } else {
        format!(
            "{}\n{prompt_message}",
            super::prompt::sent_stamp(chrono::Local::now())
        )
    };

    let memory_context = format!(
        "{}{}{}",
        memory_context,
        if shared {
            String::new()
        } else {
            crate::task::dispatch::projection(server, session_id).await
        },
        task_handoff
    );

    let soul = if shared {
        "You are Jeeves. Reply to the current request in the originating chat.".into()
    } else {
        load_soul(
            &settings.home,
            thin_soul_for(&settings.soul_mode, &resolved.provider_id),
        )
    };
    if let Some((added, removed)) = server.mcp.refresh(&settings.home).await {
        eprintln!(
            "(jeeves) MCP tools rebuilt between turns: added [{}], removed [{}]",
            added.join(", "),
            removed.join(", ")
        );
    }
    let current_tools =
        crate::connector_settings::apply(&settings.home, &server.mcp.apply(&server.tools));
    let mut turn_tools = crate::enabled_tools(&current_tools, &settings)
        .session_view_with(server.activation_set(session_id));
    if channel_chat {
        turn_tools = jeeves_tools::ToolRegistry::new();
    } else if scoped_task {
        for name in turn_tools
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
        {
            if !matches!(
                name.as_str(),
                "read_file"
                    | "glob"
                    | "grep"
                    | "write_file"
                    | "edit"
                    | "apply_patch"
                    | "run_command"
                    | "todo_write"
            ) {
                turn_tools = turn_tools.without(&name);
            }
        }
    }
    let main_chat = session_id == crate::interaction::MAIN_SESSION;
    if chat_operations.unresolved || (main_chat && frame.interaction_tool.is_none()) {
        turn_tools.restrict_to_reads(!chat_operations.unresolved);
    }
    if chat_operations.unresolved {
        eprintln!(
            "(jeeves) earlier chat operations need review; this loop permits only proven read tools"
        );
    }
    if settings.tool_policy == "deferred" {
        turn_tools.defer_all_but_always();
    }
    if main_chat
        && !chat_operations.unresolved
        && let Some(tool) = frame.interaction_tool.cloned()
    {
        if !channel_chat {
            turn_tools = turn_tools.without("schedule_job").without("recurring_work");
            turn_tools.register_exclusive_with_effect(
                crate::responsibility::reviews::tool(
                    server.clone(),
                    turn_id.into(),
                    message.into(),
                ),
                jeeves_tools::ToolEffect::Draft,
            );
        }
        turn_tools.register_exclusive(tool);
    }
    let reserve = jeeves_provider::tokens::estimate_text(&soul)
        + jeeves_provider::tokens::estimate_text(&memory_context)
        + jeeves_provider::tokens::estimate_text(&skills)
        + jeeves_provider::tokens::estimate_tools(&turn_tools.schemas())
        + SUMMARY_RESERVE_TOKENS;
    let factor = server.chat.context_factor(session_id).await.unwrap_or(None);
    let history_budget = budget_tokens(resolved, &settings, factor).saturating_sub(reserve);

    let session_write_lock = server.session_write_lock(session_id);
    let Loaded {
        messages: mut history,
        mut summary,
        stats: mut history_stats,
    } = if resume.is_some() || channel_chat {
        Loaded::empty()
    } else {
        {
            let _compact_guard = session_write_lock.lock().await;
            let loaded = interruptible(
                interrupt.as_ref(),
                "compaction",
                tokio::time::timeout(
                    PRELOOP_TIMEOUT,
                    load_history(
                        &server.chat,
                        &summary_model,
                        session_id,
                        settings.history_turns,
                        history_budget,
                        None,
                    ),
                ),
            )
            .await;
            match loaded {
                Some(Ok(loaded)) => loaded,
                Some(Err(_)) => {
                    eprintln!(
                        "(jeeves) compaction took over {}s; this turn goes without history",
                        PRELOOP_TIMEOUT.as_secs()
                    );
                    Loaded::empty()
                }
                None => Loaded::empty(),
            }
        }
    };
    let memory_index = if shared {
        String::new()
    } else {
        jeeves_memory::mirror::recall_catalog(
            &server.facts,
            &jeeves_memory::bundle::Bundle::new(&settings.home),
        )
        .await
        .unwrap_or_else(|error| {
            eprintln!("(jeeves) memory catalog unavailable: {error}");
            String::new()
        })
    };

    let instructions = turn_cwd.as_deref().and_then(crate::agents_md::discover);
    let tools_available = !turn_tools.schemas().is_empty();
    let capabilities = if channel_chat {
        Some(if frame.interaction_tool.is_some() {
            "Use task for work and refer only to tasks available in the originating chat. For a request to execute a command, hand off the exact request."
        } else {
            "Answer only the current message. The sender has no task dispatch permission."
        }.into())
    } else if scoped_task {
        Some("Work in this task's isolated directory. Report results for the chat that requested this work.".into())
    } else if main_chat && tools_available {
        let note = if turn_tools
            .get(crate::interaction::task_tool::NAME)
            .is_some()
        {
            concat!(
                "Answer conversation, advice and progress questions here. Use recurring_work to propose scheduled or event-driven work from the enabled capabilities. ",
                "Deliver the requested result here using the enabled tools and per-action approval. Include sources and clearly identify evidence you could not verify. For work requiring restart-safe follow-up, its own workspace, or independent acceptance checks, use task with the human's exact words. ",
                "A request to continue developing a project or add a feature is an assignment even when the human does not say 'task'. ",
                "Delegate before planning the implementation in chat: a missing implementation choice does not block handoff. ",
                "The worker can inspect the workspace, prepare reversible work and surface real blockers. ",
                "If the person already selected a workspace, use it instead of searching other projects. ",
                "Creating the task grants no new permission: an unconfirmed target or a material decision still gates the affected action. ",
                "Continue an identified task instead of creating a duplicate. ",
                "The task inherits the session's permissions. A handoff queues work. Report a send only on its receipt."
            )
        } else {
            "This main chat turn can use its read tools. Task handoff is unavailable for this turn."
        };
        Some(note.to_string())
    } else {
        capability_note(&settings, tools_available)
    };
    let schedule_lookup = (main_chat
        && !shared
        && !chat_operations.unresolved
        && owner_run.is_none()
        && resume.is_none()
        && turn_tools.get("search_web").is_some())
    .then(|| super::schedule_lookup::request(message, chrono::Local::now().date_naive()))
    .flatten();
    let scheduled_search = schedule_lookup.is_some();
    let prompt_for = |summary: Option<&str>| {
        let mut prompt = build_system(PromptParts {
            soul: &soul,
            model: &resolved.model,
            provider: &resolved.provider_id,
            memory: &memory_context,
            memory_index: Some(memory_index.as_str()),
            skill_index: Some(skill_index.as_str()),
            skills: &skills,
            todo: server.todo.render(session_id).as_deref(),
            summary,
            instructions: instructions.as_deref(),
            capabilities: capabilities.as_deref(),
        });
        if main_chat && turn_tools.get("search_web").is_some() {
            prompt.tail.push_str(super::prompt::WEB_LOOKUP_RULES);
        }
        if scheduled_search {
            prompt.tail.push_str(super::schedule_lookup::EVIDENCE_NOTE);
        }
        prompt.tail.push_str(&chat_operations.prompt);
        prompt.tail.push_str(server.permission_note(session_id));
        prompt
    };
    let mut prompt = prompt_for(summary.as_deref());

    let usage = crate::memory_center::usage_of(
        vec![
            (
                "soul",
                jeeves_provider::tokens::estimate_text(&prompt.stable),
            ),
            (
                "memory",
                jeeves_provider::tokens::estimate_text(&memory_context)
                    + jeeves_provider::tokens::estimate_text(&memory_index),
            ),
            (
                "skills",
                jeeves_provider::tokens::estimate_text(&skill_index)
                    + jeeves_provider::tokens::estimate_text(&skills),
            ),
            (
                "tools",
                jeeves_provider::tokens::estimate_tools(&turn_tools.schemas()),
            ),
            (
                "history",
                jeeves_provider::tokens::estimate_messages(&history),
            ),
            (
                "summary",
                summary
                    .as_deref()
                    .map(jeeves_provider::tokens::estimate_text)
                    .unwrap_or(0),
            ),
        ],
        budget_tokens(resolved, &settings, factor),
        settings.context_warn_ratio,
    );
    if let Ok(mut map) = server.context_usage.lock() {
        map.insert(session_id.to_string(), usage);
    }

    let delta_sink = sink.clone();
    let delta_turn_id = turn_id.to_string();
    let on_text: TextSink = Arc::new(move |delta: &str| {
        delta_sink.notification(ServerNotification::TextDelta(TextDeltaNotification {
            turn_id: delta_turn_id.clone(),
            delta: delta.to_string(),
        }));
    });

    let mut ctx = server.tool_ctx(
        session_id,
        turn_cwd.clone(),
        server.session_approval(session_id, turn_id, sink),
    );
    ctx.turn_limits = frame.tool_limits.clone();
    ctx.worktree = if scoped_task {
        turn_cwd.clone()
    } else {
        frame.worktree.clone()
    };
    let limits = match &run {
        Some(run) => super::accounting::active_limits(server, run, resume.as_ref())
            .await
            .map_err(|message| ErrorObject {
                code: codes::INTERNAL_ERROR,
                message,
                data: None,
            })?,
        None => None,
    };
    ctx.task_run_id = run.map(|run| run.id);
    ctx.edit_checks = Some(std::sync::Arc::new(crate::edit_check_host::HostEditChecks(
        server.clone(),
        ctx.clone(),
    )));
    ctx.activate = Some(turn_tools.activator());

    let tool_started: jeeves_loop::Observer = {
        let sink = sink.clone();
        let turn_id = turn_id.to_string();
        Arc::new(move |event| match event {
            LoopEvent::ToolStart {
                tool_use_id,
                name,
                args,
            } => {
                sink.notification(ServerNotification::ToolStarted(ToolStartedNotification {
                    turn_id: turn_id.clone(),
                    tool_use_id,
                    tool: name,
                    args,
                }));
            }
            LoopEvent::Tool {
                tool_use_id,
                name,
                output,
                success,
                duration_ms,
                ..
            } => {
                sink.notification(ServerNotification::ToolCompleted(
                    ToolCompletedNotification {
                        turn_id: turn_id.clone(),
                        tool_use_id,
                        tool: name,
                        output,
                        status: if success {
                            ToolStatus::Ok
                        } else {
                            ToolStatus::Error
                        },
                        duration_ms: Some(crate::narrow(duration_ms)),
                    },
                ));
            }
            LoopEvent::ToolDelta { tool_use_id, chunk } => {
                sink.notification(ServerNotification::ToolDelta(ToolDeltaNotification {
                    turn_id: turn_id.clone(),
                    tool_use_id,
                    chunk,
                }));
            }
            _ => {}
        })
    };
    let observer: Option<jeeves_loop::Observer> = match inner {
        Some(inner) => {
            let combined: jeeves_loop::Observer = Arc::new(move |event| {
                tool_started(event.clone());
                inner(event);
            });
            Some(combined)
        }
        None => Some(tool_started),
    };

    let mut retried = false;
    let context_window = settings.context_window.unwrap_or(resolved.context_window());
    let output_ceiling =
        server
            .catalog
            .output_ceiling(&resolved.provider_id, &resolved.model, settings.max_tokens);
    let mut initial_search = schedule_lookup.map(super::schedule_lookup::Lookup::plan);
    let mut result: LoopResult = loop {
        let context_budget = budget_tokens(resolved, &settings, factor)
            .saturating_sub(
                jeeves_provider::tokens::estimate_text(&prompt.stable)
                    + jeeves_provider::tokens::estimate_text(&prompt.tail)
                    + jeeves_provider::tokens::estimate_tools(&turn_tools.schemas()),
            )
            .max(1024);
        let attempt = jeeves_loop::run_with_schedule_search(
            jeeves_loop::Turn {
                system_prefix: Some(prompt.stable.clone()),
                client: execution.client.as_ref(),
                model: &resolved.model,
                system: prompt.tail.clone(),
                history: history.clone(),
                user_message: prompt_message.clone(),
                user_images: user_images.clone(),
                tools: &turn_tools,
                ctx: ctx.clone(),
                run_limits: limits.clone(),
                context_budget: Some(context_budget),
                context_window: Some(context_window),
                max_tokens: output_ceiling,
                token_budget: Some(settings.turn_token_budget)
                    .filter(|budget| *budget > 0)
                    .map(|budget| budget as usize),
                tool_result_budget: crate::tool_result_budget(&settings),
                observer: observer.clone(),
                on_text: (!scheduled_search).then(|| on_text.clone()),
                interrupt: interrupt.clone(),
                checkpoint: checkpoint.clone(),
                steering: steering.clone(),
                resume: resume.clone(),
            },
            initial_search.take(),
        )
        .await;

        match attempt {
            Ok(result) => break result,
            Err(e) if !retried && is_context_overflow(&e) => {
                eprintln!("(jeeves) context overflowed; compacting and retrying once");
                retried = true;
                let forced = {
                    let _compact_guard = session_write_lock.lock().await;
                    load_history(
                        &server.chat,
                        &summary_model,
                        session_id,
                        settings.history_turns,
                        0,
                        Some(FORCED_RETRY_TURNS),
                    )
                    .await
                };
                history = forced.messages;
                summary = forced.summary;
                history_stats = forced.stats;
                prompt = prompt_for(summary.as_deref());
            }
            Err(e) => {
                return Err(ErrorObject {
                    code: codes::PROVIDER_ERROR,
                    message: format!("Model call failed: {e}"),
                    data: None,
                });
            }
        }
    };

    if scheduled_search {
        result.reply = super::schedule_lookup::seat_safe_reply(&result.reply);
    }
    Ok(FullTurn {
        result,
        history: history_stats,
    })
}

/// The turn's main model wrapped in the person's fallback chain (`model-routes.json`
/// `fallback`). `meter` wraps each target on its own, so usage lands under the provider and
/// model that actually answered. With no backups configured this is just the metered primary.
fn with_fallback(
    server: &Server,
    primary: &Resolved,
    meter: impl Fn(&Resolved) -> Resolved,
) -> Resolved {
    let backups = server.fallback_targets(primary);
    let metered = meter(primary);
    if backups.is_empty() {
        return metered;
    }
    let target = |label: String, resolved: &Resolved, client, rescue_only| {
        jeeves_provider::fallback::Target {
            label,
            client,
            model: resolved.model.clone(),
            small_model: resolved.small_model.clone(),
            vision: resolved.vision,
            max_output_tokens: Some(server.catalog.output_ceiling(
                &resolved.provider_id,
                &resolved.model,
                server.settings().max_tokens,
            )),
            rescue_only,
        }
    };
    let chain = jeeves_provider::fallback::Fallback::new(
        target(
            primary.provider_id.clone(),
            primary,
            metered.client.clone(),
            false,
        ),
        backups
            .iter()
            .map(|(label, resolved, rescue)| {
                target(label.clone(), resolved, meter(resolved).client, *rescue)
            })
            .collect(),
    );
    Resolved {
        client: Arc::new(chain),
        ..metered
    }
}
