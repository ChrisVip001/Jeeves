use crate::Server;
use jeeves_protocol::{GoalRoundNotification, ServerNotification};
use jeeves_provider::{ContentBlock, Message, Resolved, Role};

/// One full loop: retrieval gate → system prompt → sliding-window history → THE LOOP.
///
/// When the graph's front door is open, the `full_agent` node calls this function — the default path
/// without the graph and the path inside the graph **share the same implementation**, so the
/// "loop as node" can never drift from the "loop as default".
///
/// `inner` is the event outlet the node leaves for the loop: when running inside the graph, events must
/// leave through it so the engine can attach `node=full_agent`. `None` when there is no graph.
///
/// Turns force-kept on overflow retry: keep the newest two turns verbatim, fold the rest into the summary.
pub(super) const FORCED_RETRY_TURNS: usize = 2;

/// Reserve for the summary section: the summary model's cap (700 tokens) + section header and headroom.
/// The summary is generated inside load_history; at budget time it does not exist yet, so it must be set aside up front.
pub(super) const SUMMARY_RESERVE_TOKENS: usize = 800;

/// This turn's context budget (tokens): window (`JEEVES_CONTEXT_WINDOW` first, else the
/// provider table's approximation) × ratio. Floor of 1024 — however small the ratio, never squeeze
/// the window below fitting even one turn of conversation.
///
/// `factor` is last turn's measured/estimated ratio (see `session_context`): **calibrate estimates with measurements**.
/// When the estimate is too small (factor > 1), shrink the budget so compaction happens sooner; relax when it is too large.
/// No record (first turn, or the graph path) means no calibration.
pub(super) fn budget_tokens(
    resolved: &Resolved,
    settings: &jeeves_config::Settings,
    factor: Option<f64>,
) -> usize {
    let window = settings
        .context_window
        .unwrap_or_else(|| resolved.context_window());
    let raw = window as f64 * settings.compact_threshold;
    let calibrated = match factor {
        Some(factor) if factor > 0.0 => raw / factor,
        _ => raw,
    };
    (calibrated as usize).max(1024)
}

/// Is this error a "context overflow"? The loop wrapped the provider error in anyhow;
/// drill down one level here to recognize it — overflow is the only error worth "retry once after compaction".
pub(super) fn is_context_overflow(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<jeeves_provider::ProviderError>()
        .is_some_and(jeeves_provider::ProviderError::is_context_overflow)
}

/// The transcript for the judge: the session's recent turns, with this turn's reply appended.
///
/// The judge **only reads the conversation** (no tools, no retrieval): what it must answer is "does this
/// conversation show the goal is met"; looking at anything else only makes it start doing the model's work.
pub(super) async fn judge_transcript(server: &Server, session_id: &str, reply: &str) -> String {
    let history = server
        .chat
        .session_history(session_id)
        .await
        .unwrap_or_default();
    if history.is_empty() {
        return format!("assistant: {reply}");
    }
    let mut out = String::new();
    let recent = history.len().saturating_sub(8);
    for (role, content) in &history[recent..] {
        out.push_str(&format!("{role}: {content}\n"));
    }
    out.push_str(&format!("assistant: {reply}"));
    out
}

/// Emit one goal-loop notification (`GoalRound` is the only answer to "why another round ran").
pub(super) fn notify_goal(
    sink: &crate::EventSink,
    turn_id: &str,
    round: i32,
    max_rounds: i32,
    status: &str,
    reason: &str,
) {
    sink.notification(ServerNotification::GoalRound(GoalRoundNotification {
        turn_id: turn_id.to_string(),
        round,
        max_rounds,
        status: status.to_string(),
        reason: reason.to_string(),
    }));
}

/// Take only the most recent N turns, plus a rolling summary.
///
/// Without a sliding window, a long session stuffs the entire history into the prompt every turn until
/// the context explodes; with only a sliding window, the squeezed-out part equals amnesia. So: the squeezed-out
/// What one turn's history (and its compaction) looked like — the numbers `traces/*.jsonl` wants.
///
/// All of them are "before/after" in **turns**, not tokens: what compaction actually decides is
/// how many turns survive, and the token estimate that drove that decision is already recorded
/// on the result (`estimatedInputTokens` / `observedInputTokens`).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct HistoryStats {
    pub(crate) turns_total: usize,
    pub(crate) turns_kept: usize,
    pub(crate) compacted: bool,
}

/// The result of loading history: what goes into the prompt, the summary, and what it cost.
pub(crate) struct Loaded {
    pub(crate) messages: Vec<Message>,
    pub(crate) summary: Option<String>,
    pub(crate) stats: HistoryStats,
}

impl Loaded {
    /// No history (first turn / resume / compaction failed). Stats are zero, which is honest.
    pub(super) fn empty() -> Self {
        Self {
            messages: Vec::new(),
            summary: None,
            stats: HistoryStats::default(),
        }
    }
}

/// old turns are handed to `compaction`, folded into one summary (fail-open; worst case a truncated excerpt),
/// stored with the session, and spliced into the system prompt every turn. Returns `(working memory, summary)`.
/// `token_budget` is the approximate token count this turn **may spend on history**; `window_override`
/// is the "force compaction after context overflow" path: it pins how many turns to keep, ignoring the budget.
pub(super) async fn load_history(
    chat: &jeeves_state::Chat,
    resolved: &Resolved,
    session_id: &str,
    history_turns: i32,
    token_budget: usize,
    window_override: Option<usize>,
) -> Loaded {
    let pairs = match chat.session_history(session_id).await {
        Ok(pairs) => pairs,
        Err(error) => {
            eprintln!("(jeeves) history unavailable for this turn: {error}");
            Vec::new()
        }
    };
    let ceiling = history_turns.max(0) as usize;
    let window = match window_override {
        Some(forced) => forced,
        None => ceiling.min(jeeves_memory::compaction::turns_that_fit(
            &pairs,
            token_budget,
        )),
    };

    let (summary, compacted) = jeeves_memory::compaction::refresh_with_status(
        chat,
        resolved.client.as_ref(),
        &resolved.small_model,
        session_id,
        &pairs,
        window,
    )
    .await
    .unwrap_or_else(|error| {
        eprintln!("(jeeves) rolling summary unavailable for this turn: {error}");
        (None, false)
    });

    let messages = pairs
        .iter()
        .rev()
        .take(window)
        .rev()
        .flat_map(|(user, assistant)| {
            vec![
                Message::user_text(user.clone()),
                Message {
                    role: Role::Assistant,
                    content: vec![ContentBlock::Text {
                        text: assistant.clone(),
                    }],
                },
            ]
        })
        .collect();
    Loaded {
        messages,
        summary,
        stats: HistoryStats {
            turns_total: pairs.len(),
            turns_kept: window,
            compacted,
        },
    }
}

/// After a turn: fold what the next turn will evict into the rolling summary now, in the
/// background, so that turn finds nothing newly evicted and makes no summary call before its own.
///
/// The next turn's real budget is not known yet (its soul, memory and tools are assembled then),
/// so this one is approximate: 90% of the budget minus the summary reserve. It can
/// only fold less than the turn would; the turn's own `refresh` covers the rest. Fail-open and
/// bounded; the monotonic save guard keeps a late run from undoing a newer summary.
pub(super) async fn precompact(server: &Server, resolved: &Resolved, session_id: &str) {
    let settings = server.settings();
    let factor = server.chat.context_factor(session_id).await.unwrap_or(None);
    let budget = (budget_tokens(resolved, &settings, factor) as f64 * 0.9) as usize;
    let budget = budget.saturating_sub(SUMMARY_RESERVE_TOKENS).max(1024);
    let lock = server.session_write_lock(session_id);
    let _guard = lock.lock().await;
    let work = load_history(
        &server.chat,
        resolved,
        session_id,
        settings.history_turns,
        budget,
        None,
    );
    if tokio::time::timeout(super::PRELOOP_TIMEOUT, work)
        .await
        .is_err()
    {
        eprintln!("(jeeves) background compaction timed out; the next turn compacts for itself");
    }
}

/// Fold what the tools did into the assistant text that gets written to `chat_log`.
///
/// This is the **only** trace of a tool call that survives into the next turn's history (the
/// conversation is text-level; tool_use/tool_result blocks are rebuilt per turn). So the line
/// has to carry three things: what ran, how much came back, and — when the output was spilled
/// to disk — **where to read it again**. Without the address a long task re-reads the same
/// files every turn, because the model can see it learned something but not what.
///
/// `spill_dir` (normally `<home>/spill`) closes the gap the in-turn spill leaves: a result
/// below that spill's threshold reached the model whole, so it carries no address — and after
/// folding only its first line would survive. Any such result longer than
/// [`FOLD_SPILL_CHARS`] is written out here so its line gains the same "full output at" address.
/// `None` folds without writing anything (the old behavior).
pub(crate) fn fold_tool_activity(
    reply: &str,
    calls: &[jeeves_loop::ToolOutcome],
    spill_dir: Option<&std::path::Path>,
) -> String {
    if calls.is_empty() {
        return reply.to_string();
    }
    let summary = calls
        .iter()
        .map(|call| fold_one(call, spill_dir))
        .collect::<Vec<_>>()
        .join("; ");
    format!("{reply}\n[tools used: {summary}]")
}

/// The reply without trailing `[tools used: …]` lines the model wrote itself.
///
/// History shows that footer at the end of every earlier reply, so models copy it, sometimes
/// with a call from a previous turn. A copied footer misleads the user, and once folded the
/// stored reply carries two, which teaches the next turn the same habit. Only
/// [`fold_tool_activity`] writes the footer, from the calls that actually ran.
pub(crate) fn strip_echoed_footer(reply: &str) -> &str {
    let mut kept = reply.trim_end();
    loop {
        let start = kept.rfind('\n').map_or(0, |i| i + 1);
        let line = kept[start..].trim();
        if !(line.starts_with("[tools used:") && line.ends_with(']')) {
            return kept;
        }
        kept = kept[..start].trim_end();
    }
}

/// A result longer than this, with no spill address of its own, is spilled when folded.
///
/// The folded line keeps 160 characters of the first line; below ~2k the rest is a few lines
/// the model can cheaply re-run, and a file per tiny result would only litter `spill/`.
pub(crate) const FOLD_SPILL_CHARS: usize = 2_000;

/// One call, one line: `name(1.2k chars, full output at spill/20261112/exec-….txt: first line…)`.
pub(super) fn fold_one(
    call: &jeeves_loop::ToolOutcome,
    spill_dir: Option<&std::path::Path>,
) -> String {
    let chars = call.output.chars().count();
    let first = call.output.lines().next().unwrap_or("").trim();
    let head = jeeves_provider::truncate_chars(first, 160);
    let address = match spill_address(&call.output) {
        Some(address) => Some(address.to_string()),
        None if chars > FOLD_SPILL_CHARS => spill_dir.and_then(|dir| {
            let spilled = jeeves_tools::spill::spill_text(dir, &call.output, "fold");
            if spilled.is_none() {
                eprintln!(
                    "(jeeves) could not keep the full output of {} ({chars} chars) for later turns; only its first line survives",
                    call.name
                );
            }
            spilled
        }),
        None => None,
    };
    match address {
        Some(address) => format!(
            "{}({} chars, full output at {address}; first line: {})",
            call.name, chars, head
        ),
        None => format!("{}({} chars: {})", call.name, chars, head),
    }
}

/// The spill address inside a tool result, if it carries one.
///
/// Parsed back out of the text instead of threaded through the loop: the text the model read
/// is exactly the text folded here, so there is one source of truth and no second field to
/// keep in sync (`jeeves_tools::spill` writes "... full output at <path>)").
pub(super) fn spill_address(output: &str) -> Option<&str> {
    /// The Chinese markers are still here on purpose: ledgers written before the wording was
    /// translated contain them, and a resume re-reads those rows.
    const MARKERS: &[&str] = &["完整输出在 ", "全量输出在 ", "full output at "];
    for marker in MARKERS {
        let Some(start) = output.rfind(marker) else {
            continue;
        };
        let rest = &output[start + marker.len()..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '）' || c == ')' || c == '，' || c == '。')
            .unwrap_or(rest.len());
        let address = rest[..end].trim();
        if !address.is_empty() {
            return Some(address);
        }
    }
    None
}
