//! The subagent's executor.
//!
//! It lives at this layer because it needs the model, state handles, and tool table — `jeeves-tools`
//! only knows the `SubagentRunner` trait (the dependency direction cannot be reversed: loop depends on tools).
//!
//! Differences from the parent turn, spelled out one by one:
//!
//! * **Its own context**: empty history + a short system (persona + one line "you do only this task"),
//!   no retrieval, no skills, no summary. A delegated task should be self-contained.
//! * **Its own token budget**: delegated work has a separate cost boundary.
//! * **No further delegation**: the tool table explicitly drops `delegate_task` (recursive delegation is
//!   the kind of thing that is invisible until someone tries it and burns through the quota).
//! * **No notifications, no interrupts**: the parent turn only knows "delegate_task was called", then gets the conclusion.
//! * **Nothing persisted**: the subagent's conversation is not written to `chat_log` (otherwise
//!   `session/list` would fill with unclaimed sessions). To know what it did, read the returned summary — it lists its tools.
//!
//! Model and settings are **read at runtime** (sharing those two Arcs, not cloning at construction):
//! a turn arriving after `config/write` switches providers, or after `install_provider` in tests,
//! must use the **current** client — capturing a snapshot from startup is a silent trap.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use jeeves_config::Settings;
use jeeves_provider::Resolved;
use jeeves_state::{Chat, Episodes, Facts};
use jeeves_tools::{BoxFut, ToolRegistry, subagent::SubagentRunner};
use serde_json::Value;

/// Per-loop cost budget for a subagent.
const SUBAGENT_TOKEN_BUDGET: Option<usize> = Some(120_000);

/// Distinguishes subagents spawned within the same millisecond (a batch spawns up to four).
static SUBAGENT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) struct Delegated {
    /// A copy of the parent turn's tool table (already without `delegate_task`).
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) mcp: Arc<crate::server::mcp_live::McpLive>,
    pub(crate) facts: Facts,
    pub(crate) episodes: Episodes,
    pub(crate) chat: Chat,
    /// Settings and model handles shared with `Server` (read at runtime; see module docs).
    pub(crate) settings: Arc<RwLock<Settings>>,
    pub(crate) resolved: Arc<RwLock<Option<Resolved>>>,
    pub(crate) catalog: Arc<jeeves_provider::catalog::Catalog>,
}

/// What gets fed to the subagent this time.
struct Prompt {
    /// The stable half, passed through to the loop untouched so the cache boundary holds.
    system_prefix: Option<String>,
    system: String,
    history: Vec<jeeves_provider::Message>,
    user_message: String,
}

/// Run one subagent loop. Extracted because "the structured result failed validation" needs **one
/// more round**, and that round is identical to the first except for history and instruction.
async fn run_child(
    resolved: &Resolved,
    tools: &ToolRegistry,
    ctx: jeeves_tools::ToolCtx,
    prompt: Prompt,
    budget: jeeves_loop::budget::ToolResultBudget,
    context_window: u32,
    output_ceiling: i32,
) -> anyhow::Result<jeeves_loop::LoopResult> {
    let prompt_overhead = jeeves_provider::tokens::estimate_text(&prompt.system)
        + prompt
            .system_prefix
            .as_deref()
            .map(jeeves_provider::tokens::estimate_text)
            .unwrap_or(0)
        + jeeves_provider::tokens::estimate_tools(&tools.schemas());
    jeeves_loop::run(jeeves_loop::Turn {
        system_prefix: prompt.system_prefix,
        client: resolved.client.as_ref(),
        model: &resolved.model,
        system: prompt.system,
        history: prompt.history,
        user_message: prompt.user_message,
        user_images: Vec::new(),
        tools,
        ctx,
        run_limits: None,
        context_budget: Some(
            (context_window as usize * 4 / 5)
                .saturating_sub(prompt_overhead)
                .max(1024),
        ),
        context_window: Some(context_window),
        max_tokens: output_ceiling,
        token_budget: SUBAGENT_TOKEN_BUDGET,
        tool_result_budget: budget,
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
}

/// Squeeze the subagent's conclusion into a "schema-valid JSON string".
fn coerce_structured(reply: &str, schema: &Value) -> Result<String, String> {
    let Some(json) = jeeves_provider::extract_json(reply) else {
        return Err("no JSON object found in the reply".to_string());
    };
    let Ok(value) = serde_json::from_str::<Value>(&json) else {
        return Err("that JSON does not parse".to_string());
    };
    jeeves_tools::validate_value(schema, &value)?;
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

impl SubagentRunner for Delegated {
    fn run(
        &self,
        task: String,
        result_schema: Option<serde_json::Value>,
        parent: jeeves_tools::ToolCtx,
        readonly: bool,
    ) -> BoxFut {
        let tools = self.tools.clone();
        let mcp = self.mcp.clone();
        let settings = self.settings.clone();
        let resolved = self.resolved.clone();
        let facts = self.facts.clone();
        let episodes = self.episodes.clone();
        let chat = self.chat.clone();
        let catalog = self.catalog.clone();
        let cwd = parent.cwd;
        let worktree = parent.worktree;

        Box::pin(async move {
            let Some(resolved) = resolved
                .read()
                .expect("the resolved lock should not be poisoned")
                .clone()
            else {
                return Err(anyhow::anyhow!(
                    "the model is not configured; a subagent cannot start"
                ));
            };
            let settings = settings
                .read()
                .expect("the settings lock should not be poisoned")
                .clone();
            let home: PathBuf = settings.home.clone();
            let tools = crate::connector_settings::apply(&home, &mcp.apply(&tools));

            let child_tools = crate::enabled_tools(&tools, &settings)
                .without(jeeves_tools::subagent::NAME)
                .without("todo_write");
            let child_tools = if readonly {
                child_tools.readonly()
            } else {
                child_tools
            };

            let soul = crate::turn::load_soul(
                &home,
                crate::turn::thin_soul_for(&settings.soul_mode, &resolved.provider_id),
            );
            let prompt = crate::turn::build_system(crate::turn::PromptParts {
                soul: &soul,
                model: &resolved.model,
                provider: &resolved.provider_id,
                memory: "",
                memory_index: None,
                skill_index: None,
                skills: "",
                todo: None,
                summary: None,
                instructions: None,
                capabilities: crate::turn::capability_note(
                    &settings,
                    !child_tools.schemas().is_empty(),
                )
                .as_deref(),
            });
            let system_prefix = Some(prompt.stable);
            let system = prompt.tail
                + "\n\nYou are a subagent: do the one task below and answer with the \
                   conclusion only. Whoever asked cannot see your steps, and you cannot ask \
                   follow-up questions."
                + if readonly {
                    "\n\nYou can only READ in this task: writing files and running commands are \
                     not available (several subagents are running at once). If the job needs a \
                     change, say so in the conclusion instead of attempting it."
                } else {
                    ""
                };

            let child_session = format!(
                "subagent:{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
                SUBAGENT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let owner_turn: Option<String> = sqlx::query_scalar(
                "SELECT c.turn_id FROM turn_checkpoints c JOIN tool_operations o ON o.turn_id=c.turn_id AND o.round_id=c.round_id
                 WHERE c.session_id=? AND o.run_id IS ? AND o.status='intent' AND o.tool='delegate_task'
                 AND ?=o.round_id||':'||(c.iterations+1)||':'||o.tool_use_id"
            ).bind(&parent.session_id).bind(&parent.task_run_id).bind(&parent.invocation_id).fetch_optional(&chat.pool()).await?;
            anyhow::ensure!(
                parent.task_run_id.is_none() || owner_turn.is_some(),
                "Missing parent execution for delegated request"
            );
            let resolved = crate::turn::meter::metered(
                &chat.pool(),
                &resolved,
                &parent.session_id,
                owner_turn.as_deref().unwrap_or(&child_session),
                parent.task_run_id.as_deref(),
                "delegation",
            );
            let mut ctx = crate::tool_ctx(
                &facts,
                &episodes,
                &chat,
                &home,
                crate::Injections {
                    session_id: child_session.clone(),
                    cwd,
                    jobs: None,
                    approval: None,
                    computer_use: None,
                },
            );

            ctx.worktree = worktree;
            let budget = crate::tool_result_budget(&settings);
            let context_window = settings.context_window.unwrap_or(resolved.context_window());
            let output_ceiling =
                catalog.output_ceiling(&resolved.provider_id, &resolved.model, settings.max_tokens);

            let result = run_child(
                &resolved,
                &child_tools,
                ctx.clone(),
                Prompt {
                    system_prefix: system_prefix.clone(),
                    system: system.clone(),
                    history: Vec::new(),
                    user_message: task,
                },
                budget,
                context_window,
                output_ceiling,
            )
            .await
            .map_err(|e| anyhow::anyhow!("Model call failed: {e}"))?;

            let mut summary = result.reply.trim().to_string();

            if let Some(schema) = &result_schema {
                match coerce_structured(&summary, schema) {
                    Ok(json) => summary = json,
                    Err(why) => {
                        let nudge = format!(
                            "The previous answer cannot be used as a structured result: {why}\nReply with **only** JSON that matches the schema below (no code fence, no extra explanation):\n{schema}"
                        );
                        let retry = run_child(
                            &resolved,
                            &child_tools,
                            ctx,
                            Prompt {
                                system_prefix: system_prefix.clone(),
                                system,
                                history: result.messages.clone(),
                                user_message: nudge,
                            },
                            budget,
                            context_window,
                            output_ceiling,
                        )
                        .await;
                        match retry {
                            Ok(second) => match coerce_structured(second.reply.trim(), schema) {
                                Ok(json) => {
                                    eprintln!(
                                        "(jeeves) the subagent's structured result passed on the retry"
                                    );
                                    summary = json;
                                }
                                Err(again) => {
                                    eprintln!(
                                        "(jeeves) the subagent's structured result failed validation twice: {again}"
                                    );
                                    summary.push_str(&format!(
                                        "\n(the structured result failed validation twice: {again}; above is its raw text)"
                                    ));
                                }
                            },
                            Err(e) => {
                                summary.push_str(&format!(
                                    "\n(the structured retry did not run: {e})"
                                ));
                            }
                        }
                    }
                }
            }
            if summary.is_empty() {
                summary = "(the subagent reached no conclusion.)".to_string();
            }
            let used: Vec<String> = result
                .tool_calls
                .iter()
                .map(|call| {
                    if call.ok() {
                        call.name.clone()
                    } else {
                        format!("{} (failed)", call.name)
                    }
                })
                .collect();
            if !used.is_empty() {
                summary.push_str(&format!(
                    "\n(the subagent ran {} rounds and used: {})",
                    result.iterations,
                    used.join("、")
                ));
            }
            Ok(summary)
        })
    }
}
