//! THE LOOP — observe → reason → act → repeat. This file is the whole trick.
//!
//! Every agent framework, stripped to the bone, is this loop plus more indirection:
//!
//! ```text
//! while not done:
//!     response = llm(messages, tools)     # reason
//!     if response wants tools:
//!         results = run(tool_calls)       # act
//!         messages += results             # observe
//!     else:
//!         done                            # reply to the human
//! ```
//!
//! The loop continues while the model requests tools or the user adds input.
//! Completion, cancellation, exhausted resources and stalled work end the loop.
//!
//! The observer is an enum of events, not a `(kind, dict)` — fields have types,
//! so a misspelled key is a compile-time error.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use jeeves_provider::{
    ContentBlock, CreateRequest, Message, Provider, Role, StopReason, TextSink, Usage,
};
use jeeves_tools::{ToolCtx, ToolRegistry};
use serde_json::Value;

pub mod budget;
pub mod guard;
mod projection;

use budget::ToolResultBudget;
use guard::{GuardReport, StallGuard};

#[cfg(test)]
#[path = "guard_tests.rs"]
mod guard_tests;

#[cfg(test)]
#[path = "budget_tests.rs"]
mod budget_tests;

/// The turn's interrupt switch. `turn/interrupt` flips it down from another task;
/// the loop stands down at the next safe point (model call, or tool execution racing a select).
///
/// No tokio_util CancellationToken: one atomic bit plus a Notify is enough — not worth
/// pulling in a whole dependency for one flagpole.
#[derive(Debug, Default)]
pub struct Interrupt {
    cancelled: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

impl Interrupt {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Flip the switch down. Flipping it again after cancel is idempotent.
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Returns at the moment of cancellation. Suspends waiting for the notify otherwise.
    pub async fn wait(&self) {
        while !self.is_cancelled() {
            let notified = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Words the human added **while this turn was already running**.
///
/// The loop folds them into the working memory at the next iteration boundary, so tools that
/// already ran stay run and their results stay valid — that is the whole difference between
/// steering and "cancel and start over". One list, two readers: the loop consumes what it has
/// not seen yet (`drain_pending`), app-server reads `all` afterwards to write the transcript,
/// so a message accepted right before the turn ended is still recorded rather than lost.
#[derive(Debug, Default)]
pub struct Steering {
    messages: Vec<String>,
    /// How many of `messages` the loop has already folded in. A counter, not `remove(0)`:
    /// app-server must still see the earlier ones when it writes the log.
    consumed: usize,
}

impl Steering {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }

    /// Accept one addition. Called from another task (`turn/steer`), hence `&mut`.
    pub fn push(&mut self, text: impl Into<String>) {
        self.messages.push(text.into());
    }

    /// Everything not yet folded into the working memory, in arrival order.
    pub fn drain_pending(&mut self) -> Vec<String> {
        let pending = self.messages[self.consumed..].to_vec();
        self.consumed = self.messages.len();
        pending
    }

    fn has_pending(&self) -> bool {
        self.consumed < self.messages.len()
    }

    /// Everything ever accepted for this turn — the transcript writes all of it.
    pub fn all(&self) -> Vec<String> {
        self.messages.clone()
    }
}

/// Events the loop sends out. The gateway renders them, trace persists them —
/// neither needs to be wired into the loop's logic.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopEvent {
    /// One model call completed.
    Llm {
        iteration: i32,
        stop_reason: StopReason,
        usage: Usage,
    },
    /// One delta of streamed text. Only appears when `on_text` is passed.
    Text { delta: String },
    /// One tool **started** (result not out yet — the Tool event comes later).
    ///
    /// `tool_use_id` is the id the model minted for this call. Several calls can be in flight
    /// at once (read-only ones share a batch), so it — not the name — is what a client keys
    /// on when it has to place an update on the right row.
    ToolStart {
        tool_use_id: String,
        name: String,
        args: Value,
    },
    /// Live output from a tool that is **still running** (`run_command` pipes its stdout).
    ///
    /// Progress only: the Tool event that follows carries the real, complete result, and that
    /// is what the model sees. A consumer that ignores this event loses nothing but liveness.
    ToolDelta { tool_use_id: String, chunk: String },
    /// One tool returned. Uncertain external effects report unsuccessful completion.
    Tool {
        tool_use_id: String,
        name: String,
        args: Value,
        output: String,
        success: bool,
        duration_ms: i64,
    },
}

/// Observer: every loop event passes through it. Arc instead of a reference,
/// so it can be moved together with `on_text` into a 'static closure.
pub type Observer = Arc<dyn Fn(LoopEvent) + Send + Sync>;

/// One boundary snapshot: the working memory as the model is about to be called again —
/// assistant thoughts, tool calls and their results all in `messages`, nothing half-done.
#[derive(Debug, Clone)]
pub struct ToolCheckpoint {
    pub proposed_only: bool,
    pub tool_use_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    pub output: Option<jeeves_tools::ToolOutput>,
}

/// The current tool batch only; prior conversation is never admitted as new work.
#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub operations: Vec<ToolCheckpoint>,
    pub round_id: String,
    pub messages: Vec<Message>,
    /// Completed iterations so far (the tool round just fed back included).
    pub iterations: i32,
    /// Token usage spent so far (a resumed turn adds both halves up).
    pub usage: Usage,
}

/// The checkpoint outlet, called at each tool-results boundary. Awaiting on purpose:
/// the write must land **before** the next model call, or a crash loses exactly the
/// boundary it was supposed to save.
pub type CheckpointSink = Arc<
    dyn Fn(
            Checkpoint,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Resume from a checkpoint: the loop continues an existing working memory instead of
/// assembling history + user message — finished tools are never re-run.
#[derive(Debug, Clone)]
pub struct Resume {
    pub round_id: String,
    pub messages: Vec<Message>,
    /// How many iterations were already completed before the crash.
    pub completed_iterations: i32,
    /// Token usage already spent before the crash.
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    /// The id the model minted for this call (matches its `tool_use` block) — what a client
    /// uses to settle the row it opened on `ToolStart`.
    pub tool_use_id: String,
    pub name: String,
    pub input: serde_json::Value,
    pub output: String,
    pub success: bool,
    pub duration_ms: i64,
}

impl ToolOutcome {
    pub fn ok(&self) -> bool {
        self.success
    }
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub stop_reason: StopReason,
    pub token_budget_reached: bool,
    pub stop_detail: Option<String>,
    pub round_id: String,
    /// What the model says to the user.
    pub reply: String,
    pub tool_calls: Vec<ToolOutcome>,
    pub iterations: i32,
    /// Token usage for the whole turn (summed over every LLM call).
    pub usage: Usage,
    /// The full working memory — assistant thoughts, tool calls, tool results all in here;
    /// exactly what trace wants.
    pub messages: Vec<Message>,
    /// True when interrupted by `turn/interrupt`: reply holds text already streamed before
    /// the interrupt, not guaranteed complete.
    pub interrupted: bool,
    /// This turn's loop-guard tally (hit count, last note).
    pub guard: GuardReport,
    /// The **locally estimated** input tokens of the last request (0 = none sent yet).
    /// Only paired with the provider-reported `usage` to calibrate the next turn — not the ledger.
    pub estimated_input_tokens: usize,
    /// The **measured** input tokens of the last call (provider-reported prefill).
    /// Two numbers for the same request as `estimated_input_tokens`: dividing gives the error.
    pub observed_input_tokens: usize,
}

/// How many times one turn may ask the model to carry on after its reply hit the output limit.
/// Two, then the cut is reported: a reply that still does not fit needs a different plan.
const MAX_OUTPUT_CONTINUATIONS: u32 = 2;

/// Sent after a reply that hit the output limit, so the model finishes instead of the turn
/// ending on half an answer.
const CONTINUE_AFTER_LIMIT: &str = "(Your last reply hit the output token limit and was cut off; any tool call in it did not run.) \
Continue exactly where it stopped. Do not apologize, recap or repeat what you already wrote. \
If you were writing a large file or command, split the work into smaller pieces.";

/// Stands in for a cut-off reply that kept no text, so the transcript stays well formed.
const CUT_OFF_PLACEHOLDER: &str = "(This reply was cut off by the output limit.)";

/// Sent once per turn after the model ended its turn with no text and no tool call (a bare
/// thinking block, or nothing after a tool round), so the person is not left with an empty reply.
const ANSWER_AFTER_EMPTY: &str = "(Your last reply contained no visible text and no tool call, so the user saw nothing.) \
Reply to the user now in plain text. If tool results above already answer the request, report them; \
if something failed or is missing, say so.";

/// Replaces an empty reply's blank text in the transcript, so the nudge after it is well formed
/// (some wires reject an empty text block outright).
const EMPTY_PLACEHOLDER: &str = "(This reply had no visible text.)";

/// What the person sees when the model stays empty after the nudge.
const EMPTY_AFTER_NUDGE: &str = "(The model ended this turn without any text, even after being asked to answer. \
Try sending the request again, or switch to another model.)";

/// A tool-free reply can report progress when a turn exhausts its token budget.
const WRAP_UP_TOKENS: &str = "(Budget note: this turn's token budget is spent; the tools have been put away.)\
Answer in three sentences: what is done, where you are stuck, what you need from me next —\
and say plainly that the stop came from this turn's token budget (JEEVES_TURN_TOKEN_BUDGET), not from the work being finished.\
If you have not verified something, say you are unsure — do not invent results.";

pub struct Turn<'a> {
    pub client: &'a dyn Provider,
    pub model: &'a str,
    /// The **stable prefix** of the system prompt: soul, model identity, the method
    /// paragraph. Identical turn after turn within a session, so the provider's prompt
    /// cache can serve it at a fraction of the input price (Anthropic needs the explicit
    /// `cache_control` boundary; the wires with automatic caching just need it not to move).
    /// `None` = one undivided system prompt.
    pub system_prefix: Option<String>,
    /// The per-turn tail: clock (day granularity), todo list, memory hits, skill bodies,
    /// the rolling summary. It changes every turn by nature, so it stays outside the cache
    /// boundary — marking it would invalidate the prefix on every write.
    pub system: String,
    /// History from previous turns (sliding-window trimmed). The new message is appended at the end.
    pub history: Vec<Message>,
    pub user_message: String,
    /// Image blocks riding on this turn's user message (attachments, already read and
    /// base64'd by app-server). Empty = plain text turn, zero overhead. They ride only
    /// on the **first** user message — later rounds of the same turn carry tool results.
    pub user_images: Vec<ContentBlock>,
    pub tools: &'a ToolRegistry,
    pub ctx: ToolCtx,
    pub run_limits: Option<budget::RunLimits>,
    pub context_budget: Option<usize>,
    pub context_window: Option<u32>,
    /// Catalog and operator ceiling. Fit against each request's actual projected input.
    pub max_tokens: i32,
    /// Cost cap for this turn in tokens (`JEEVES_TURN_TOKEN_BUDGET`). `None`/0 = off.
    ///
    /// The budget can reserve a final tool-free response.
    pub token_budget: Option<usize>,
    /// Budget for intra-turn tool results (off by default; see `budget.rs`).
    pub tool_result_budget: ToolResultBudget,
    /// Every event passes through it — the gateway renders, trace persists.
    /// Arc instead of a reference, so it can move with `on_text` into a 'static closure.
    pub observer: Option<Arc<dyn Fn(LoopEvent) + Send + Sync>>,
    /// Pass it for streaming: one callback per text chunk the model emits.
    /// Runs fine without it — the provider falls back to non-streaming.
    pub on_text: Option<TextSink>,
    /// Pass it to support interrupts: each model call and tool execution races "cancel".
    pub interrupt: Option<Arc<Interrupt>>,
    /// Called at each tool-results boundary — the durable "how far did this turn get".
    pub checkpoint: Option<CheckpointSink>,
    /// Adds words to this turn mid-flight (`turn/steer`). `None` = no steering channel.
    pub steering: Option<Arc<Mutex<Steering>>>,
    /// Resume instead of starting fresh: `user_message`, `history` and `user_images` are
    /// ignored, the loop continues from the checkpointed working memory.
    pub resume: Option<Resume>,
}

/// Group one round's calls into batches that may run together.
///
/// A read-only call joins the batch before it; an exclusive call is its own single-call
/// batch, which makes it a barrier in both directions. Request order is preserved — the
/// round's results are assembled in that order regardless of completion order.
fn plan_segments(
    tools: &ToolRegistry,
    uses: Vec<(&str, &str, &Value)>,
) -> Vec<Vec<(String, String, Value)>> {
    let mut segments: Vec<Vec<(String, String, Value)>> = Vec::new();
    let mut open_shared = false;
    for (id, name, input) in uses {
        let exclusive = tools.is_exclusive(name) || !tools.effect(name).is_read();
        if !exclusive
            && open_shared
            && let Some(last) = segments.last_mut()
        {
            last.push((id.to_string(), name.to_string(), input.clone()));
            continue;
        }
        segments.push(vec![(id.to_string(), name.to_string(), input.clone())]);
        open_shared = !exclusive;
    }
    segments
}

/// Recovery sees only the prefix that has reached dispatch and the results already observed.
fn admitted_exchange(
    messages: &[Message],
    admitted: usize,
    results: &[ContentBlock],
) -> Vec<Message> {
    let mut snapshot = messages.to_vec();
    let mut remaining = admitted;
    snapshot
        .last_mut()
        .expect("assistant response")
        .content
        .retain(|block| {
            if matches!(block, ContentBlock::ToolUse { .. }) {
                if remaining == 0 {
                    return false;
                }
                remaining -= 1;
            }
            true
        });
    if !results.is_empty() {
        snapshot.push(Message {
            role: Role::User,
            content: results.to_vec(),
        });
    }
    snapshot
}

pub async fn run(turn: Turn<'_>) -> Result<LoopResult> {
    run_with_schedule_search(turn, None).await
}

pub type ScheduleRefinement = Box<dyn FnOnce(&str) -> Option<Value> + Send>;
pub type ScheduleFollow = Box<dyn Fn(&ToolOutcome) -> Option<Value> + Send>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScheduleTool {
    Search,
    Fetch,
}

impl ScheduleTool {
    fn name(self) -> &'static str {
        match self {
            Self::Search => "search_web",
            Self::Fetch => "fetch_url",
        }
    }
}

/// A host-selected lookup, one optional search refinement, and one original dated link.
pub struct ScheduleSearch {
    pub initial: (ScheduleTool, Value),
    pub refine: Option<ScheduleRefinement>,
    pub follow: ScheduleFollow,
}

/// Run scheduled reads through normal admission, checkpointing and observation.
pub async fn run_with_schedule_search(
    turn: Turn<'_>,
    schedule: Option<ScheduleSearch>,
) -> Result<LoopResult> {
    run_inner(turn, schedule, false).await
}

/// A read-only research mode can still deliver observed, host-rendered excerpts when the
/// provider stops. Preserve tool results, counters and usage instead of discarding the round.
pub async fn run_research(turn: Turn<'_>) -> Result<LoopResult> {
    run_inner(turn, None, true).await
}

async fn run_inner(
    turn: Turn<'_>,
    schedule: Option<ScheduleSearch>,
    retain_reads_on_provider_error: bool,
) -> Result<LoopResult> {
    let schedule = schedule.filter(|schedule| {
        turn.resume.is_none()
            && turn.run_limits.is_none()
            && turn.tools.get(schedule.initial.0.name()).is_some()
    });
    let (mut pending, mut refine, mut follow) = match schedule {
        Some(ScheduleSearch {
            initial,
            refine,
            follow,
        }) => (Some(initial), refine, Some(follow)),
        None => (None, None, None),
    };
    let round_id = turn
        .resume
        .as_ref()
        .map(|r| r.round_id.clone())
        .unwrap_or_else(|| {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            format!(
                "round-{}-{stamp}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        });
    let notify: Option<Observer> = turn.observer.clone();
    let (mut messages, mut usage, completed_before) = match &turn.resume {
        Some(resume) => (
            resume.messages.clone(),
            resume.usage,
            resume.completed_iterations,
        ),
        None => {
            let mut messages = turn.history.clone();
            let mut user_content = vec![ContentBlock::Text {
                text: turn.user_message.clone(),
            }];
            user_content.extend(turn.user_images.iter().cloned());
            messages.push(Message {
                role: Role::User,
                content: user_content,
            });
            (messages, Usage::default(), 0)
        }
    };
    if let Some(checkpoint) = &turn.checkpoint {
        checkpoint(Checkpoint {
            round_id: round_id.clone(),
            messages: messages.clone(),
            iterations: completed_before,
            operations: Vec::new(),
            usage,
        })
        .await
        .map_err(jeeves_provider::ProviderError::Api)?;
    }
    let active_from = if turn.resume.is_some() {
        messages
            .iter()
            .position(|m| {
                m.role == Role::User
                    && !turn.user_message.is_empty()
                    && m.text().contains(&turn.user_message)
            })
            .unwrap_or(0)
    } else {
        turn.history.len()
    };
    let mut context_budget = turn.context_budget;
    let token_ceiling = turn
        .token_budget
        .filter(|cap| *cap > 0)
        .into_iter()
        .chain(
            turn.run_limits
                .as_ref()
                .and_then(|limits| limits.tokens)
                .map(|cap| cap.max(0) as usize),
        )
        .min();
    let mut compaction_reported = false;
    let completed_tools = if turn.run_limits.is_some() && turn.resume.is_some() {
        messages
            .iter()
            .flat_map(|message| &message.content)
            .filter(|block| matches!(block, ContentBlock::ToolUse { .. }))
            .count()
    } else {
        0
    };
    let mut tool_calls: Vec<ToolOutcome> = Vec::new();
    let spill_dir = turn.ctx.home.join("spill");
    let mut last_estimate = 0usize;
    let mut last_observed = 0usize;
    let mut sent_len = 0usize;
    let mut guard = StallGuard::new();

    let streamed: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
    let record: TextSink = {
        let streamed = streamed.clone();
        Arc::new(move |delta: &str| {
            streamed
                .lock()
                .expect("the streamed lock should not be poisoned")
                .push_str(delta);
        })
    };

    let mut iterations = completed_before;
    let mut stalled_batches = 0;
    let mut continuations = 0u32;
    let mut continued_reply = String::new();
    let mut empty_nudged = false;
    let stop_detail = 'research_round: loop {
        let iteration = iterations.saturating_add(1);
        if turn
            .interrupt
            .as_ref()
            .is_some_and(|cancel| cancel.is_cancelled())
        {
            return Ok(interrupted_result(Scene {
                round_id: round_id.clone(),
                streamed,
                tool_calls,
                iterations: iteration - 1,
                usage,
                messages,
                guard: GuardReport::close(&guard),
                estimated_input_tokens: last_estimate,
                observed_input_tokens: last_observed,
            }));
        }

        if let Some(reason) = turn
            .run_limits
            .as_ref()
            .and_then(|limits| limits.blocker(usage, completed_tools + tool_calls.len(), 0))
        {
            break reason;
        }

        if let Some(steering) = &turn.steering {
            let pending = steering
                .lock()
                .expect("the steering lock should not be poisoned")
                .drain_pending();
            for text in pending {
                messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text { text }],
                });
            }
        }

        let spent = usage
            .input_tokens
            .max(0)
            .saturating_add(usage.output_tokens.max(0)) as usize;
        let mut request_messages = messages.clone();
        let trim = budget::trim_tool_results(
            &mut request_messages,
            Some(&spill_dir),
            &turn.tool_result_budget,
            &turn.ctx.session_id,
        );
        let mut projected = trim.trimmed > 0;
        if let Some(compacted) = context_budget
            .filter(|limit| jeeves_provider::tokens::estimate_messages(&request_messages) > *limit)
            .and_then(|limit| projection::compact(&request_messages, active_from, limit))
        {
            if !compaction_reported {
                eprintln!(
                    "(jeeves) compacting active model requests without discarding checkpoints"
                );
                compaction_reported = true;
            }
            request_messages = compacted;
            projected = true;
        }
        let system_tokens = jeeves_provider::tokens::estimate_text(&turn.system)
            + turn
                .system_prefix
                .as_deref()
                .map(jeeves_provider::tokens::estimate_text)
                .unwrap_or(0);
        let mut schemas = turn.tools.schemas();
        let next_input = if projected {
            jeeves_provider::tokens::estimate_messages(&request_messages)
                + system_tokens
                + jeeves_provider::tokens::estimate_tools(&schemas)
        } else {
            let appended = jeeves_provider::tokens::estimate_messages(
                messages.get(sent_len..).unwrap_or_default(),
            );
            budget::next_input_tokens(last_observed, appended, last_estimate)
        };
        let out_of_tokens = budget::should_wrap_up(turn.token_budget, spent, next_input);
        if out_of_tokens {
            schemas.clear();
        }
        let mut preflight = if out_of_tokens { None } else { pending.take() };
        let preflight_tool = preflight.as_ref().map(|(tool, _)| *tool);
        let mut recovered_overflow = false;
        let sampled = loop {
            if let Some((tool, input)) = preflight.take() {
                break Some(jeeves_provider::CreateResponse {
                    stop_reason: StopReason::ToolUse,
                    usage: Usage::default(),
                    content: vec![ContentBlock::ToolUse {
                        id: format!("schedule-{round_id}-{iteration}"),
                        name: tool.name().into(),
                        input,
                        extra: None,
                    }],
                });
            }
            let mut model_messages = request_messages.clone();
            if out_of_tokens {
                model_messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::Text {
                        text: WRAP_UP_TOKENS.to_string(),
                    }],
                });
            }
            last_estimate = jeeves_provider::tokens::estimate_messages(&model_messages)
                + system_tokens
                + jeeves_provider::tokens::estimate_tools(&schemas);
            let fitted = budget::fit_output(turn.max_tokens, turn.context_window, last_estimate);
            let max_tokens = token_ceiling.map_or(fitted, |cap| {
                let remaining = cap.saturating_sub(spent).saturating_sub(last_estimate);
                (fitted as usize).min(remaining.max(16)) as i32
            });
            let request = CreateRequest {
                cache_prefix: turn.system_prefix.clone(),
                model: turn.model.to_string(),
                system: Some(turn.system.clone()),
                messages: model_messages,
                tools: schemas.clone(),
                max_tokens,
            };
            match sample(&turn, request, notify.clone(), record.clone()).await {
                Some(Ok(response)) => break Some(response),
                Some(Err(error))
                    if error.is_context_overflow() && (iterations > 0 || turn.resume.is_some()) =>
                {
                    let budget = jeeves_provider::tokens::estimate_messages(&request_messages) / 2;
                    let compacted = (!recovered_overflow)
                        .then(|| projection::compact(&messages, active_from, budget))
                        .flatten()
                        .filter(|projection| {
                            jeeves_provider::tokens::estimate_messages(projection)
                                < jeeves_provider::tokens::estimate_messages(&request_messages)
                        });
                    let Some(compacted) = compacted else {
                        break None;
                    };
                    eprintln!("(jeeves) compacted the active context after provider overflow");
                    context_budget = Some(budget);
                    request_messages = compacted;
                    recovered_overflow = true;
                }
                Some(Err(_)) if retain_reads_on_provider_error => {
                    break 'research_round "The research model stopped before the round finished"
                        .into();
                }
                Some(Err(error)) => return Err(error.into()),
                None => {
                    return Ok(interrupted_result(Scene {
                        round_id: round_id.clone(),
                        streamed,
                        tool_calls,
                        iterations,
                        usage,
                        messages,
                        guard: GuardReport::close(&guard),
                        estimated_input_tokens: last_estimate,
                        observed_input_tokens: last_observed,
                    }));
                }
            }
        };
        let Some(response) = sampled else {
            break "The active context exceeded the model window after compaction. Completed work was saved".into();
        };

        iterations = iteration;
        usage.input_tokens += response.usage.input_tokens;
        usage.output_tokens += response.usage.output_tokens;
        usage.cache_read_input_tokens += response.usage.cache_read_input_tokens;
        usage.cache_creation_input_tokens += response.usage.cache_creation_input_tokens;
        last_observed = response.usage.prompt_size().max(0) as usize;
        sent_len = messages.len();
        if preflight_tool.is_none()
            && let Some(notify) = &notify
        {
            notify(LoopEvent::Llm {
                iteration,
                stop_reason: response.stop_reason,
                usage: response.usage,
            });
        }

        let uses = response.tool_uses();
        let tools_allowed = !out_of_tokens && response.stop_reason != StopReason::MaxTokens;
        if tools_allowed
            && let Some(reason) = turn.run_limits.as_ref().and_then(|limits| {
                limits.blocker(usage, completed_tools + tool_calls.len(), uses.len())
            })
            && !uses.is_empty()
        {
            break reason;
        }
        let mut kept: Vec<ContentBlock> = response
            .content
            .iter()
            .filter(|block| tools_allowed || !matches!(block, ContentBlock::ToolUse { .. }))
            .cloned()
            .collect();
        let cut_off = response.stop_reason == StopReason::MaxTokens && !out_of_tokens;
        if cut_off && kept.is_empty() {
            kept.push(ContentBlock::Text {
                text: CUT_OFF_PLACEHOLDER.to_string(),
            });
        }
        messages.push(Message {
            role: Role::Assistant,
            content: kept,
        });

        if cut_off && continuations < MAX_OUTPUT_CONTINUATIONS {
            continuations += 1;
            continued_reply.push_str(&response.text());
            eprintln!(
                "(jeeves) the reply hit the output limit; asking the model to continue ({continuations}/{MAX_OUTPUT_CONTINUATIONS})"
            );
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: CONTINUE_AFTER_LIMIT.to_string(),
                }],
            });
            continue;
        }

        if tools_allowed
            && uses.is_empty()
            && response.stop_reason == StopReason::EndTurn
            && turn.steering.as_ref().is_some_and(|steer| {
                steer
                    .lock()
                    .expect("the steering lock should not be poisoned")
                    .has_pending()
            })
        {
            continue;
        }
        let silent = tools_allowed
            && uses.is_empty()
            && response.stop_reason == StopReason::EndTurn
            && continued_reply.trim().is_empty()
            && response.text().trim().is_empty();
        if silent && !empty_nudged {
            empty_nudged = true;
            eprintln!("(jeeves) the model ended its turn with no text; asking it once to answer");
            if let Some(last) = messages.last_mut() {
                last.content.retain(
                    |block| !matches!(block, ContentBlock::Text { text } if text.trim().is_empty()),
                );
                last.content.push(ContentBlock::Text {
                    text: EMPTY_PLACEHOLDER.to_string(),
                });
            }
            messages.push(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: ANSWER_AFTER_EMPTY.to_string(),
                }],
            });
            continue;
        }
        if uses.is_empty() || !tools_allowed {
            let reply = if silent {
                eprintln!(
                    "(jeeves) the model stayed empty after the nudge; ending the turn with a notice"
                );
                EMPTY_AFTER_NUDGE.to_string()
            } else {
                format!("{continued_reply}{}", response.text())
            };
            return Ok(LoopResult {
                stop_reason: response.stop_reason,
                token_budget_reached: out_of_tokens,
                stop_detail: None,
                round_id: round_id.clone(),
                reply,
                tool_calls,
                iterations: iteration,
                usage,
                messages,
                interrupted: false,
                guard: GuardReport::close(&guard),
                estimated_input_tokens: last_estimate,
                observed_input_tokens: last_observed,
            });
        }

        let mut ids = std::collections::HashSet::new();
        if uses
            .iter()
            .any(|(id, _, _)| id.is_empty() || !ids.insert(*id))
        {
            anyhow::bail!("Duplicate or empty tool call identity");
        }
        let proposed = uses.len();
        let proposals: Vec<_> = uses
            .iter()
            .map(|(id, name, input)| ToolCheckpoint {
                tool_use_id: (*id).to_owned(),
                tool: (*name).to_owned(),
                input: (*input).clone(),
                proposed_only: true,
                output: None,
            })
            .collect();
        let mut admitted = 0;
        let mut results = Vec::new();
        let mut progressed = false;
        let uncertain = Arc::new(std::sync::atomic::AtomicBool::new(false));
        for segment in plan_segments(turn.tools, uses) {
            if let Some(cancel) = turn.interrupt.as_ref()
                && cancel.is_cancelled()
            {
                return Ok(interrupted_result(Scene {
                    round_id: round_id.clone(),
                    streamed,
                    tool_calls,
                    iterations: iteration - 1,
                    usage,
                    messages,
                    guard: GuardReport::close(&guard),
                    estimated_input_tokens: last_estimate,
                    observed_input_tokens: last_observed,
                }));
            }

            admitted += segment.len();
            let mut operations: Vec<_> = segment
                .iter()
                .map(|(id, name, input)| ToolCheckpoint {
                    proposed_only: false,
                    tool_use_id: id.clone(),
                    tool: name.clone(),
                    input: input.clone(),
                    output: None,
                })
                .collect();
            let checkpoint =
                |iterations, results: &[ContentBlock], operations: &[ToolCheckpoint]| {
                    turn.checkpoint.as_ref().map(|sink| {
                        sink(Checkpoint {
                            round_id: round_id.clone(),
                            messages: admitted_exchange(&messages, admitted, results),
                            iterations,
                            operations: operations.to_vec(),
                            usage,
                        })
                    })
                };
            let mut admissions = operations.clone();
            if admitted == segment.len() {
                admissions.extend_from_slice(&proposals[admitted..]);
            }
            if let Some(save) = checkpoint(iteration - 1, &results, &admissions) {
                save.await.map_err(jeeves_provider::ProviderError::Api)?;
            }
            for (id, name, input) in &segment {
                if let Some(notify) = &notify {
                    notify(LoopEvent::ToolStart {
                        tool_use_id: id.clone(),
                        name: name.clone(),
                        args: input.clone(),
                    });
                }
            }
            let mut set = tokio::task::JoinSet::new();
            for (position, (id, name, input)) in segment.iter().enumerate() {
                let registry = turn.tools.clone();
                let mut ctx = turn.ctx.clone();
                ctx.invocation_id = Some(format!("{round_id}:{iteration}:{id}"));
                let uncertain = uncertain.clone();
                let host_notice = ctx.on_uncertain.clone();
                ctx.on_uncertain = Some(Arc::new(move |reason| {
                    uncertain.store(true, std::sync::atomic::Ordering::Release);
                    if let Some(notice) = &host_notice {
                        notice(reason);
                    }
                }));
                if let Some(notify) = &notify {
                    let notify = notify.clone();
                    let id = id.clone();
                    ctx.on_delta = Some(Arc::new(move |chunk: &str| {
                        notify(LoopEvent::ToolDelta {
                            tool_use_id: id.clone(),
                            chunk: chunk.to_string(),
                        });
                    }));
                }
                let name = name.clone();
                let input = input.clone();
                set.spawn(async move {
                    let started = Instant::now();
                    let output = registry.execute_result(ctx, &name, input).await;
                    (position, output, started.elapsed().as_millis() as i64)
                });
            }

            let mut slots: Vec<Option<(jeeves_tools::ToolOutput, i64)>> = vec![None; segment.len()];
            let mut panic_note: Option<String> = None;
            let mut completed_reads = vec![None; segment.len()];
            loop {
                let next = match turn.interrupt.as_ref() {
                    Some(cancel) => tokio::select! {
                            _ = cancel.wait() => {
                                for (position, slot) in slots.iter().enumerate() {
                                    if let (Some((output, duration_ms)), Some((id, name, input))) =
                                        (slot, segment.get(position))
                                    {
                                        tool_calls.push(ToolOutcome {
                                            tool_use_id: id.clone(),
                                            name: name.clone(),
                                            input: input.clone(),
                                            output: output.text.clone(),
                                            success: output.success,
                                            duration_ms: *duration_ms,
                                        });
                                    }
                                }
                                return Ok(interrupted_result(Scene {
                    round_id: round_id.clone(),
                                    streamed,
                                    tool_calls,
                                    iterations: iteration - 1,
                                    usage,
                                    messages,
                                    guard: GuardReport::close(&guard),
                                    estimated_input_tokens: last_estimate,
                                    observed_input_tokens: last_observed,
                                }));
                            }
                            next = set.join_next() => next,
                        },
                    None => set.join_next().await,
                };
                match next {
                    Some(Ok((position, output, duration_ms))) => {
                        if segment.len() > 1 {
                            let op = &mut operations[position];
                            op.output = Some(output.clone());
                            completed_reads[position] = Some(ContentBlock::ToolResult {
                                tool_use_id: op.tool_use_id.clone(),
                                content: output.text.clone(),
                            });
                            let ordered: Vec<_> = results
                                .iter()
                                .chain(completed_reads.iter().flatten())
                                .cloned()
                                .collect();
                            if let Some(save) =
                                checkpoint(iteration - 1, &ordered, std::slice::from_ref(op))
                            {
                                save.await.map_err(jeeves_provider::ProviderError::Api)?;
                            }
                            if let Some(notify) = &notify {
                                notify(LoopEvent::Tool {
                                    tool_use_id: op.tool_use_id.clone(),
                                    name: op.tool.clone(),
                                    args: op.input.clone(),
                                    output: output.text.clone(),
                                    success: output.success,
                                    duration_ms,
                                });
                            }
                        }
                        slots[position] = Some((output, duration_ms));
                    }
                    Some(Err(e)) => {
                        panic_note = Some(format!("Error: the tool task ended abnormally: {e}"))
                    }
                    None => break,
                }
            }
            if let Some(note) = panic_note {
                if !segment
                    .iter()
                    .all(|(_, name, _)| turn.tools.effect(name).is_read())
                {
                    uncertain.store(true, std::sync::atomic::Ordering::Release);
                    if let Some(notice) = &turn.ctx.on_uncertain {
                        notice(&note);
                    }
                }
                for slot in slots.iter_mut().filter(|slot| slot.is_none()) {
                    *slot = Some((jeeves_tools::ToolOutput::failure(note.clone()), 0));
                }
            }

            for (position, slot) in slots.into_iter().enumerate() {
                let Some((output, duration_ms)) = slot else {
                    continue;
                };
                let (id, name, input) = &segment[position];
                operations[position].output = Some(output.clone());
                let success =
                    output.success && !uncertain.load(std::sync::atomic::Ordering::Acquire);
                let output = output.text;
                if preflight_tool == Some(ScheduleTool::Search)
                    && let Some(refine) = refine.take()
                    && success
                    && turn.ctx.turn_limits.search_max > 1
                {
                    pending = refine(&output).map(|input| (ScheduleTool::Search, input));
                }
                let outcome = ToolOutcome {
                    tool_use_id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    output: output.clone(),
                    success,
                    duration_ms,
                };
                if success
                    && name == ScheduleTool::Fetch.name()
                    && pending.is_none()
                    && let Some(next) = follow.as_ref().and_then(|follow| follow(&outcome))
                {
                    pending = Some((ScheduleTool::Fetch, next));
                    follow = None;
                }
                tool_calls.push(outcome);
                let verdict = guard.observe(name, input, &output, success);
                progressed |= !verdict.is_stall();
                let for_model = guard::for_model(&output, name, input, &verdict);
                results.push(ContentBlock::ToolResult {
                    tool_use_id: id.clone(),
                    content: for_model,
                });
            }
            let unresolved = uncertain.load(std::sync::atomic::Ordering::Acquire);
            if !unresolved {
                let completed = if admitted == proposed {
                    iteration
                } else {
                    iteration - 1
                };
                if let Some(save) = checkpoint(completed, &results, &operations) {
                    save.await.map_err(jeeves_provider::ProviderError::Api)?;
                }
            }
            if let Some(notify) = &notify {
                let calls = &tool_calls[tool_calls.len() - segment.len()..];
                for ((call, op), already_reported) in
                    calls.iter().zip(&operations).zip(&completed_reads)
                {
                    if already_reported.is_none() {
                        notify(LoopEvent::Tool {
                            tool_use_id: call.tool_use_id.clone(),
                            name: call.name.clone(),
                            args: op.input.clone(),
                            output: call.output.clone(),
                            success: call.success,
                            duration_ms: call.duration_ms,
                        });
                    }
                }
            }
            if unresolved {
                break;
            }
        }
        messages.push(Message {
            role: Role::User,
            content: results,
        });

        if uncertain.load(std::sync::atomic::Ordering::Acquire) {
            eprintln!("(jeeves) tool outcome unknown; stopping this round for reconciliation");
            return Ok(LoopResult {
                round_id: round_id.clone(),
                stop_reason: StopReason::UncertainEffect,
                token_budget_reached: false,
                stop_detail: None,
                reply:
                    "A tool's outcome is unknown. Reconcile the external state before continuing."
                        .into(),
                tool_calls,
                iterations: iteration,
                usage,
                messages,
                interrupted: false,
                guard: GuardReport::close(&guard),
                estimated_input_tokens: last_estimate,
                observed_input_tokens: last_observed,
            });
        }

        stalled_batches = if progressed { 0 } else { stalled_batches + 1 };
        if stalled_batches >= 3 {
            break "Tools kept returning the same results after repeated warnings. Work stopped for review".into();
        }
    };
    eprintln!("(jeeves) agent loop stopped: {stop_detail}");
    Ok(LoopResult {
        stop_reason: StopReason::Other,
        token_budget_reached: false,
        reply: format!("({stop_detail}.)"),
        stop_detail: Some(stop_detail),
        round_id,
        tool_calls,
        iterations,
        usage,
        messages,
        interrupted: false,
        guard: GuardReport::close(&guard),
        estimated_input_tokens: last_estimate,
        observed_input_tokens: last_observed,
    })
}

async fn sample(
    turn: &Turn<'_>,
    request: CreateRequest,
    notify: Option<Observer>,
    record: TextSink,
) -> Option<std::result::Result<jeeves_provider::CreateResponse, jeeves_provider::ProviderError>> {
    let fut = match (turn.on_text.clone(), notify.clone()) {
        (Some(sink), Some(observer)) => {
            let (record, streamed_sink) = (record.clone(), sink.clone());
            let combined: TextSink = Arc::new(move |delta: &str| {
                record(delta);
                observer(LoopEvent::Text {
                    delta: delta.to_string(),
                });
                streamed_sink(delta);
            });
            turn.client.stream(request, combined)
        }
        (Some(sink), None) => {
            let (record, record_only) = (record.clone(), sink.clone());
            let chained: TextSink = Arc::new(move |delta: &str| {
                record(delta);
                record_only(delta);
            });
            turn.client.stream(request, chained)
        }
        (None, Some(observer)) => {
            let record = record.clone();
            let observer_only: TextSink = Arc::new(move |delta: &str| {
                record(delta);
                observer(LoopEvent::Text {
                    delta: delta.to_string(),
                });
            });
            turn.client.stream(request, observer_only)
        }
        (None, None) => turn.client.create(request),
    };

    match turn.interrupt.as_ref() {
        Some(cancel) => tokio::select! {
            _ = cancel.wait() => None,
            response = fut => Some(response),
        },
        None => Some(fut.await),
    }
}

/// The stand-down result on interrupt: reply is text already streamed (or one explanatory line),
/// and `interrupted: true` lets the caller persist it honestly, not pass off half an answer.
/// The stand-down scene assembles the interruption result. Past
/// seven or eight params, a struct reads better, and adding a field no longer breaks four sites.
struct Scene {
    round_id: String,
    streamed: Arc<Mutex<String>>,
    tool_calls: Vec<ToolOutcome>,
    iterations: i32,
    usage: Usage,
    messages: Vec<Message>,
    guard: GuardReport,
    estimated_input_tokens: usize,
    observed_input_tokens: usize,
}

fn interrupted_result(scene: Scene) -> LoopResult {
    let Scene {
        round_id,
        streamed,
        tool_calls,
        iterations,
        usage,
        messages,
        guard,
        estimated_input_tokens,
        observed_input_tokens,
    } = scene;
    let partial = streamed
        .lock()
        .expect("the streamed lock should not be poisoned")
        .clone();
    let reply = if partial.trim().is_empty() {
        "(this turn was interrupted.)".to_string()
    } else {
        partial
    };
    LoopResult {
        stop_reason: StopReason::Other,
        token_budget_reached: false,
        stop_detail: None,
        round_id,
        reply,
        tool_calls,
        iterations,
        usage,
        messages,
        interrupted: true,
        guard,
        estimated_input_tokens,
        observed_input_tokens,
    }
}

/// An empty observer, saving callers a closure when they need no events.
#[cfg(test)]
#[path = "loop_tests.rs"]
mod loop_tests;
