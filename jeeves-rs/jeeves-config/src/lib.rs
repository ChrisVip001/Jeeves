//! `jeeves-config` — every knob is an environment variable; see the root `.env.example`.
//!
//! Note Jeeves does **not** read the `.env` file: that template is for humans to source
//! (`set -a; source .env; set +a`), not loaded automatically.
//!
//! No settings framework: read once at startup, then it's an immutable plain struct.
//! If you can read this file, you know what Jeeves can be configured into.

pub mod browser;
mod json_file;
pub mod models;
pub mod routes;
pub mod vault;
pub mod workspaces;

use std::path::PathBuf;

use jeeves_protocol::SettingsView;

fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn env_bool(name: &str) -> bool {
    matches!(env(name).as_deref(), Some("1") | Some("true") | Some("yes"))
}

fn env_int(name: &str, default: i32) -> i32 {
    env(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_float(name: &str, default: f64) -> f64 {
    env(name).and_then(|v| v.parse().ok()).unwrap_or(default)
}

pub struct Bound {
    /// Field name in `SettingsPatch` (used by the `config/write` path).
    /// `None` = settable only via env vars (currently the two timeouts).
    pub patch_name: Option<&'static str>,
    pub env_name: &'static str,
    pub min: i64,
    pub max: i64,
}

pub const BOUNDS: &[Bound] = &[
    Bound {
        patch_name: Some("maxIterations"),
        env_name: "JEEVES_MAX_ITERATIONS",
        min: 1,
        max: 100,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_AUTO_CONTINUE_MAX",
        min: 0,
        max: 5,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TASK_WORKERS",
        min: 1,
        max: 8,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TURN_STEP_SLICE",
        min: 1,
        max: 60,
    },
    Bound {
        patch_name: Some("maxTokens"),
        env_name: "JEEVES_MAX_TOKENS",
        min: 128,
        max: 200_000,
    },
    Bound {
        patch_name: Some("historyTurns"),
        env_name: "JEEVES_HISTORY_TURNS",
        min: 0,
        max: 1_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_RESUME_MAX_AGE",
        min: 0,
        max: 10_080,
    },
    Bound {
        patch_name: Some("consolidateEvery"),
        env_name: "JEEVES_CONSOLIDATE_EVERY",
        min: 1,
        max: 1_000,
    },
    Bound {
        patch_name: Some("retrievalTopK"),
        env_name: "JEEVES_RETRIEVAL_TOP_K",
        min: 1,
        max: 100,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_LEDGER_INLINE_MAX",
        min: 1_024,
        max: 1_048_576,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_MAX_TURNS",
        min: 0,
        max: 64,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_HOME_MAX_MB",
        min: 0,
        max: 10_000_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TOOL_RESULT_TOTAL_CHARS",
        min: 0,
        max: 4_000_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TOOL_RESULT_MAX_CHARS",
        min: 0,
        max: 1_000_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TURN_TOKEN_BUDGET",
        min: 0,
        max: 2_000_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_TOOL_RESULT_KEEP_RECENT",
        min: 0,
        max: 50,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_SKILL_INDEX_TOKENS",
        min: 32,
        max: 2_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_SKILL_BODY_TOKENS",
        min: 64,
        max: 8_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_CONTEXT_WARN_RATIO",
        min: 50,
        max: 99,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_APPROVAL_TIMEOUT",
        min: 5,
        max: 3_600,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_GOAL_MAX_ROUNDS",
        min: 1,
        max: 20,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_LLM_RETRIES",
        min: 0,
        max: 5,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_THINKING_BUDGET",
        min: 0,
        max: 200_000,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_LLM_TIMEOUT",
        min: 1,
        max: 3_600,
    },
    Bound {
        patch_name: None,
        env_name: "JEEVES_EXEC_TIMEOUT",
        min: 1,
        max: 3_600,
    },
];

/// Count and per-entry length caps for allow rules. Rules are handwritten; hundreds of
/// entries or thousands of chars can only be an accidental paste (e.g. a whole script).
const MAX_EXEC_RULES: usize = 64;
const MAX_EXEC_RULE_CHARS: usize = 200;

/// Look up a bound by patch field name or env var name and error out.
fn check_bound(key: &str, value: i64) -> Result<(), String> {
    let bound = BOUNDS
        .iter()
        .find(|b| b.env_name == key || b.patch_name == Some(key))
        .expect("no such knob in the bounds table — that is a code bug, not a misconfiguration");
    if value < bound.min || value > bound.max {
        return Err(format!(
            "{key} must be between {} and {}, got {value}",
            bound.min, bound.max
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub provider: String,
    /// Explicit overrides: key, endpoint, model id. Empty = provider's own defaults.
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Cheap model: used by the retrieval gate and consolidation.
    pub small_model: Option<String>,
    /// Whether the **selected model** accepts images. `None` = take the catalog row's
    /// default; `Some(true)` = this model sees even on a vendor whose default does not
    /// (deepseek-v4.1-flash next to deepseek-v4-pro), `Some(false)` = text-only. Filled
    /// by `models::apply_profile` — a per-model property, not a per-vendor one.
    pub vision: Option<bool>,
    pub model_capabilities: Option<jeeves_protocol::ModelCapabilities>,

    pub home: PathBuf,

    /// `JEEVES_GOAL_MAX_ROUNDS`: max auto-continuation rounds per goal (default 5). Beyond
    /// that, honestly set `round-limit` — no faking completion, no clearing the goal.
    pub goal_max_rounds: i32,
    /// `JEEVES_PLAN`: only when enabled does a task ask the model for an execution plan before
    /// its first round, and re-plan after a failed round. Off by default: it is one more model
    /// call per task and it changes the prompt — a capability pressed by hand, like every other.
    pub plan_enabled: bool,
    /// `JEEVES_LLM_RETRIES`: max backoff retries on rate limits / transient faults (default
    /// 2, cap 5, 0 = off). Only retries errors where "try again" makes sense, and
    /// **always sends a notification per retry** — see retry.rs in jeeves-provider.
    pub llm_retries: i32,
    /// `JEEVES_THINKING_BUDGET`: Anthropic extended-thinking budget in tokens for the main model
    /// (0 = off, the default). Thinking tokens count against `max_tokens`.
    pub thinking_budget: i32,
    /// Timeout for a single model call. A hung network call must never silently freeze a turn.
    pub llm_timeout_secs: i64,
    pub max_iterations: i32,
    /// Leave headroom: reasoning models (kimi-k3, gpt-5.x, gemini-*-pro) spend output tokens
    /// thinking first; a low cap leaves stop_reason=max_tokens stuck mid-thought.
    pub max_tokens: i32,
    /// Working-memory sliding window: only the last N turns go into the prompt. Older
    /// ones stay in state.db, recovered via the retrieval gate + episodic memory.
    ///
    /// It is an **upper bound**, not a trigger: tokens decide when compaction happens,
    /// see `compact_threshold`.
    pub history_turns: i32,
    /// `JEEVES_CONTEXT_WINDOW`: overrides the approximate context window in the provider
    /// table. Local models (ollama / LM Studio) vary wildly; the table holds common defaults.
    pub context_window: Option<u32>,
    /// `JEEVES_COMPACT_THRESHOLD`: fraction of the context window used before compacting
    /// (default 0.8). Turns are the cap, tokens are the gate — one long tool output can
    /// blow the window while the turn count still looks "safe".
    pub compact_threshold: f64,

    /// `JEEVES_RESUME_MAX_AGE`: how old an `open` checkpoint may be, in **minutes**, before
    /// startup stops resuming it on its own. `0` = never stop (the old unconditional rule).
    ///
    /// A checkpoint exists for crash recovery, so the honest window is seconds to minutes.
    /// Past an hour it stops being "the same turn continued": "today", "just now" and
    /// "this week" have moved, the human's intent has usually cooled, and the resume spends
    /// a real model call to answer a question they no longer have. The stale row is marked
    /// `abandoned` with one line on stderr — asking again is one sentence.
    pub resume_max_age_minutes: i32,

    /// Distill facts only once every N turns of new conversation.
    pub consolidate_every: i32,
    /// `JEEVES_LEDGER_INLINE_MAX`: how large one ledger payload may be before it is spilled
    /// to `<home>/ledger/`. The record keeps a pointer + digest instead of the bytes —
    /// big enough that ordinary messages stay inline, small enough that a 200 KB tool
    /// output never lands in the database.
    pub ledger_inline_max: i32,
    /// `JEEVES_MAX_TURNS`: how many turns may run at once across all sessions; a turn over the
    /// cap is refused with a clear error, never queued silently. 0 = no cap.
    pub max_concurrent_turns: i32,
    /// `JEEVES_TASK_WORKERS`: how many human-started tasks may run side by side (1–8, default 1).
    /// Each running task holds a lease; a task whose worker dies is swept once, never twice.
    pub task_workers: i32,
    /// `JEEVES_HOME_MAX_MB`: how large the home directory may grow before new turns are refused
    /// (reads and cleanup keep working). 0 = no cap.
    pub home_max_mb: i32,
    pub retrieval_top_k: i32,

    /// `JEEVES_SOUL_MODE`: how much of the built-in soul scaffolding wraps `SOUL.md`.
    ///
    /// * `auto` (default) — thin for models that need no hand-holding, full for local
    ///   small models (see `PROVIDERS` / catalog: reasoning-class ids get the thin one).
    /// * `thin` — the user's `SOUL.md` plus one paragraph of method; no examples, no rules
    ///   the strong model already follows.
    /// * `full` — the original long scaffolding, unchanged.
    ///
    /// The reasoning: a strong model follows ten rules as well as forty, and every extra
    /// line is paid for on every turn. A 7B local model needs the forty.
    pub soul_mode: String,
    /// `JEEVES_SKILL_INDEX_TOKENS`: cap on the always-present skill catalog
    /// (name + one-line description per skill).
    pub skill_index_tokens: i32,
    /// `JEEVES_SKILL_BODY_TOKENS`: cap on one selected skill's body. Beyond it the text is
    /// truncated with an honest marker, never silently dropped.
    pub skill_body_tokens: i32,
    /// `JEEVES_TOOL_POLICY`: which tool schemas ride on every request.
    ///
    /// * `eager` (default) — every registered tool, as before.
    /// * `deferred` — only the always-there few; the rest are activated in-session by
    ///   `find_tools`. Tools gated by a switch or by approval (`run_command`,
    ///   `write_file`, `edit`, `delegate_task`) are **never** deferred: they stay exactly
    ///   where the switch put them.
    pub tool_policy: String,
    /// `JEEVES_CONTEXT_WARN_RATIO`: percentage of the window at which the turn emits a
    /// context-pressure notice (default 85). A notice, not a dialog: it never interrupts.
    pub context_warn_ratio: i32,

    /// `JEEVES_OPS_HOST`: address the cockpit binds to. Default 127.0.0.1 — a personal assistant
    /// isn't exposed by default; viewing remotely is **another decision**: set `JEEVES_OPS_TOKEN` too.
    pub ops_host: String,
    /// `JEEVES_OPS_TOKEN`: Bearer token for the cockpit's `/api/*` (constant-time compare).
    /// Binding a non-loopback address without it = rejected at startup, not a warning. It is
    /// **deliberately not in `view()`**: a deployment param the model must not see in payloads.
    pub ops_token: Option<String>,
    /// `JEEVES_DISPATCHERS`: sender ids (Feishu `open_id`s) allowed to hand out tasks from a chat
    /// group. Empty = nobody outside the local views. Like `ops_token`, not shown to the model.
    pub dispatchers: Vec<String>,

    /// `JEEVES_TOOL_RESULT_TOTAL_CHARS`: total char cap for all tool results within a turn.
    /// When exceeded, replace from the largest with stubs (full text spilled to disk +
    /// head/tail previews). Default 200000 — only pathological output hits it; 0 disables.
    pub tool_result_total_chars: i64,
    /// `JEEVES_TOOL_RESULT_MAX_CHARS`: a result must exceed this to qualify for stubbing.
    /// Default 30000 (about 8k tokens). Set 0 to disable.
    pub tool_result_max_chars: i64,
    /// `JEEVES_TOOL_RESULT_KEEP_RECENT`: how many of the **newest** results are never stubbed
    /// (default 3). Trimming goes oldest-first: the model is reasoning about the last few
    /// results, and stubbing one of those answers a question it just asked.
    pub tool_result_keep_recent: i32,
    /// `JEEVES_TURN_TOKEN_BUDGET`: cost cap for **one turn**, in tokens (input + output, as the
    /// provider reports it). Reaching it ends the turn the same way the step cap does — tools
    /// away, one wrap-up call — so a runaway turn costs a bounded amount instead of a night.
    /// **0 = off** (the default): the step cap already bounds rounds, and a token cap nobody
    /// asked for would cut honest work short.
    pub turn_token_budget: i64,
    /// `JEEVES_AUTO_CONTINUE_MAX`: how many times one turn may **continue itself** after it would
    /// have ended. Two reasons qualify: the human's Stop hook demanded it, or the turn was cut off
    /// by the step cap while the todo list still had `pending`/`in_progress` items.
    ///
    /// A turn that ends of its own accord is **not** continued, even with work left on the list:
    /// "here is the plan" is a finished turn, and driving it on would execute a plan nobody asked
    /// to execute.
    ///
    /// Default 2: a turn that stops one step short is the most common failure mode, and
    /// "your move" is a poor answer when the model itself wrote down what was left. 0 disables
    /// self-continuation entirely (the old behavior).
    pub auto_continue_max: i32,
    /// `JEEVES_TURN_STEP_SLICE`: extra steps granted by **one** continuation.
    ///
    /// The step cap stops being a cliff: running out means "here is another slice", as long as
    /// `auto_continue_max` still allows it. Slice rather than raise the cap, because a bigger
    /// `JEEVES_MAX_ITERATIONS` also makes a *runaway* turn more expensive — a slice is only spent
    /// when it is asked for.
    pub turn_step_slice: i32,

    /// `JEEVES_NOTIFY`: only when enabled does Jeeves read `<home>/notify.json` and push
    /// notifications to its webhooks (approval requests / job completion / blocked goals).
    /// Off by default — sending data to external services is a decision pressed by hand.
    pub notify_enabled: bool,
    /// `JEEVES_MORNING_BRIEFING`: opt-in channel and IANA timezone, e.g. outbox@Asia/Shanghai.
    pub morning_briefing: Option<String>,

    /// `JEEVES_DELEGATE`: only when enabled is `delegate_task` registered in the tool table.
    /// Off by default — "hand the work off and let it run" is capability as well as risk
    /// (one more context, one more unread trail); the switch too must be pressed by hand.
    pub delegate_enabled: bool,
    /// `JEEVES_EXEC`: only when enabled is run_command registered in the tool table. Off
    /// by default — an assistant that can execute commands must be switched on by hand.
    pub browser: Option<browser::BrowserRuntime>,
    /// Explicitly installed native window adapter.
    pub desktop_helper: Option<std::path::PathBuf>,
    pub exec_enabled: bool,
    /// `JEEVES_WRITE`: only when enabled are write_file / edit registered in the tool table.
    /// Off by default — an assistant that can change files must be switched on by hand;
    /// the writable boundary (workdir + home + JEEVES_EXEC_WRITABLE_ROOTS) is fixed at startup.
    pub write_enabled: bool,
    /// `JEEVES_EXEC_ALLOW`: allow rules (comma-separated; trailing `*` wildcard supported).
    /// **Empty list = nothing is allowed**: deny by default, not allow by default.
    pub exec_allow: Vec<String>,
    /// `JEEVES_EXEC_TIMEOUT`: timeout for a single command (seconds).
    pub exec_timeout_secs: i64,

    /// `JEEVES_APPROVAL`: what happens when nothing matches the allow table.
    ///
    /// * `never` (default) — reject outright (same as the old behavior).
    /// * `on-request` — ask once, then decide; **no answer counts as rejection**.
    ///
    /// The hard-deny list and the sandbox are **never negotiable**: approval only opens the gate.
    pub approval: String,
    /// `JEEVES_APPROVAL_TIMEOUT`: seconds to wait for an answer (default 120).
    pub approval_timeout_secs: i64,
    pub web_push_contact: Option<String>,
    /// `JEEVES_EXEC_NETWORK`: whether the sandbox **may reach the network**. Default **false
    /// (offline)** — an allowed command should not by default own the ability to ship data
    /// out. Commands like `cargo test` needing downloads require explicitly setting it to 1.
    pub exec_network: bool,
    /// `JEEVES_EXEC_WRITABLE_ROOTS`: extra writable dirs beyond workdir and home (colon-separated
    /// absolute paths, must already exist). For build caches like `cargo build`.
    pub exec_writable_roots: Vec<PathBuf>,
    /// `JEEVES_WRITE_ALLOW_DIRS`: directories whose writes **need no confirmation** in
    /// `on-request` mode — what "remember this one" writes for `write_file`/`edit`.
    ///
    /// It never widens the writable boundary: containment is checked first, so a directory listed
    /// here but outside `JEEVES_EXEC_WRITABLE_ROOTS`/workdir unlocks nothing.
    pub write_allow_dirs: Vec<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "anthropic".to_string(),
            api_key: None,
            base_url: None,
            model: None,
            small_model: None,
            vision: None,
            model_capabilities: None,
            home: PathBuf::from(".jeeves"),
            goal_max_rounds: 5,
            plan_enabled: false,
            llm_retries: 2,
            thinking_budget: 0,
            llm_timeout_secs: 120,
            max_iterations: 32,
            max_tokens: 16_384,
            history_turns: 12,
            context_window: None,
            compact_threshold: 0.8,
            resume_max_age_minutes: 60,
            consolidate_every: 6,
            ledger_inline_max: 32_768,
            max_concurrent_turns: 0,
            task_workers: 1,
            home_max_mb: 0,
            retrieval_top_k: 4,
            soul_mode: "auto".to_string(),
            skill_index_tokens: 200,
            skill_body_tokens: 1_200,
            tool_policy: "eager".to_string(),
            context_warn_ratio: 85,
            ops_host: "127.0.0.1".to_string(),
            ops_token: None,
            dispatchers: Vec::new(),
            notify_enabled: false,
            morning_briefing: None,
            delegate_enabled: false,
            tool_result_total_chars: 200_000,
            tool_result_max_chars: 30_000,
            turn_token_budget: 0,
            auto_continue_max: 2,
            turn_step_slice: 12,
            tool_result_keep_recent: 3,
            browser: None,
            desktop_helper: None,
            exec_enabled: false,
            write_enabled: false,
            exec_allow: Vec::new(),
            exec_timeout_secs: 30,
            approval: "never".to_string(),
            approval_timeout_secs: 120,
            web_push_contact: None,
            exec_network: false,
            exec_writable_roots: Vec::new(),
            write_allow_dirs: Vec::new(),
        }
    }
}

impl Settings {
    /// Read from env vars — the only read path for runtime parameters; no config file, no rules.
    ///
    /// **The exception is models and keys**: they are not configured via env vars but
    /// via `<home>/models.json` + the system keychain (see models.rs). Only `provider`
    /// is kept here as the fallback vendor when nothing is configured, so local models
    /// like ollama can start zero-config; `model` / `base_url` / `api_key` stay empty,
    /// to be filled by `models::apply_active`.
    pub fn from_env() -> Self {
        let d = Settings::default();
        Self {
            provider: env("JEEVES_PROVIDER").unwrap_or(d.provider),
            api_key: None,
            base_url: None,
            model: None,
            small_model: None,
            vision: None,
            model_capabilities: None,
            home: env("JEEVES_HOME").map(PathBuf::from).unwrap_or(d.home),
            goal_max_rounds: env_int("JEEVES_GOAL_MAX_ROUNDS", d.goal_max_rounds),
            plan_enabled: env_bool("JEEVES_PLAN"),
            llm_retries: env_int("JEEVES_LLM_RETRIES", d.llm_retries),
            thinking_budget: env_int("JEEVES_THINKING_BUDGET", d.thinking_budget),
            llm_timeout_secs: env_int("JEEVES_LLM_TIMEOUT", d.llm_timeout_secs as i32) as i64,
            max_iterations: env_int("JEEVES_MAX_ITERATIONS", d.max_iterations),
            max_tokens: env_int("JEEVES_MAX_TOKENS", d.max_tokens),
            history_turns: env_int("JEEVES_HISTORY_TURNS", d.history_turns),
            context_window: env("JEEVES_CONTEXT_WINDOW").and_then(|v| v.parse().ok()),
            compact_threshold: env_float("JEEVES_COMPACT_THRESHOLD", d.compact_threshold),
            resume_max_age_minutes: env_int("JEEVES_RESUME_MAX_AGE", d.resume_max_age_minutes),
            consolidate_every: env_int("JEEVES_CONSOLIDATE_EVERY", d.consolidate_every),
            ledger_inline_max: env_int("JEEVES_LEDGER_INLINE_MAX", d.ledger_inline_max),
            max_concurrent_turns: env_int("JEEVES_MAX_TURNS", d.max_concurrent_turns),
            task_workers: env_int("JEEVES_TASK_WORKERS", d.task_workers),
            home_max_mb: env_int("JEEVES_HOME_MAX_MB", d.home_max_mb),
            retrieval_top_k: env_int("JEEVES_RETRIEVAL_TOP_K", d.retrieval_top_k),
            soul_mode: env("JEEVES_SOUL_MODE").unwrap_or(d.soul_mode),
            skill_index_tokens: env_int("JEEVES_SKILL_INDEX_TOKENS", d.skill_index_tokens),
            skill_body_tokens: env_int("JEEVES_SKILL_BODY_TOKENS", d.skill_body_tokens),
            tool_policy: env("JEEVES_TOOL_POLICY").unwrap_or(d.tool_policy),
            context_warn_ratio: env_int("JEEVES_CONTEXT_WARN_RATIO", d.context_warn_ratio),
            ops_host: env("JEEVES_OPS_HOST").unwrap_or(d.ops_host),
            ops_token: env("JEEVES_OPS_TOKEN"),
            dispatchers: env("JEEVES_DISPATCHERS")
                .map(|raw| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            notify_enabled: env_bool("JEEVES_NOTIFY"),
            morning_briefing: env("JEEVES_MORNING_BRIEFING"),
            delegate_enabled: env_bool("JEEVES_DELEGATE"),
            tool_result_total_chars: env_int(
                "JEEVES_TOOL_RESULT_TOTAL_CHARS",
                d.tool_result_total_chars as i32,
            ) as i64,
            tool_result_max_chars: env_int(
                "JEEVES_TOOL_RESULT_MAX_CHARS",
                d.tool_result_max_chars as i32,
            ) as i64,
            tool_result_keep_recent: env_int(
                "JEEVES_TOOL_RESULT_KEEP_RECENT",
                d.tool_result_keep_recent,
            ),
            turn_token_budget: env_int("JEEVES_TURN_TOKEN_BUDGET", d.turn_token_budget as i32)
                as i64,
            auto_continue_max: env_int("JEEVES_AUTO_CONTINUE_MAX", d.auto_continue_max),
            turn_step_slice: env_int("JEEVES_TURN_STEP_SLICE", d.turn_step_slice),
            browser: browser::from_env(),
            desktop_helper: env("JEEVES_DESKTOP_HELPER").map(Into::into),
            exec_enabled: env_bool("JEEVES_EXEC"),
            write_enabled: env_bool("JEEVES_WRITE"),
            exec_allow: env("JEEVES_EXEC_ALLOW")
                .map(|raw| {
                    raw.split(',')
                        .map(str::trim)
                        .filter(|rule| !rule.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            exec_timeout_secs: env_int("JEEVES_EXEC_TIMEOUT", d.exec_timeout_secs as i32) as i64,
            approval: env("JEEVES_APPROVAL").unwrap_or(d.approval),
            web_push_contact: env("JEEVES_WEB_PUSH_CONTACT"),
            approval_timeout_secs: env_int(
                "JEEVES_APPROVAL_TIMEOUT",
                d.approval_timeout_secs as i32,
            ) as i64,
            exec_network: env_bool("JEEVES_EXEC_NETWORK"),
            exec_writable_roots: env("JEEVES_EXEC_WRITABLE_ROOTS")
                .map(|raw| {
                    std::env::split_paths(&raw)
                        .filter(|p| !p.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            write_allow_dirs: env("JEEVES_WRITE_ALLOW_DIRS")
                .map(|raw| {
                    std::env::split_paths(&raw)
                        .filter(|p| !p.as_os_str().is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Ensure home and its subdirectories exist.
    pub fn ensure_home(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(self.home.join("traces"))?;
        std::fs::create_dir_all(self.home.join("outbox"))?;
        std::fs::create_dir_all(self.home.join("skills"))?;
        std::fs::create_dir_all(self.home.join("spill"))?;
        Ok(())
    }

    /// Startup validation: illegal values **error out on the spot**, never silently default.
    ///
    /// The caller (`app-server`'s `open`) exits on `Err` and prints the reason — "failing to
    /// start" is far cheaper and far easier to debug than "runs but misbehaves".
    pub fn validate(&self) -> Result<(), String> {
        check_bound("JEEVES_MAX_ITERATIONS", self.max_iterations as i64)?;
        check_bound("JEEVES_MAX_TOKENS", self.max_tokens as i64)?;
        check_bound("JEEVES_HISTORY_TURNS", self.history_turns as i64)?;
        check_bound("JEEVES_CONSOLIDATE_EVERY", self.consolidate_every as i64)?;
        check_bound("JEEVES_RESUME_MAX_AGE", self.resume_max_age_minutes as i64)?;
        check_bound("JEEVES_LEDGER_INLINE_MAX", self.ledger_inline_max as i64)?;
        check_bound("JEEVES_MAX_TURNS", self.max_concurrent_turns as i64)?;
        check_bound("JEEVES_TASK_WORKERS", self.task_workers as i64)?;
        check_bound("JEEVES_HOME_MAX_MB", self.home_max_mb as i64)?;
        check_bound("JEEVES_RETRIEVAL_TOP_K", self.retrieval_top_k as i64)?;
        check_bound("JEEVES_GOAL_MAX_ROUNDS", self.goal_max_rounds as i64)?;
        check_bound("JEEVES_LLM_RETRIES", self.llm_retries as i64)?;
        check_bound("JEEVES_THINKING_BUDGET", self.thinking_budget as i64)?;
        check_bound("JEEVES_LLM_TIMEOUT", self.llm_timeout_secs)?;
        check_bound("JEEVES_EXEC_TIMEOUT", self.exec_timeout_secs)?;
        check_bound("JEEVES_APPROVAL_TIMEOUT", self.approval_timeout_secs)?;
        check_bound(
            "JEEVES_TOOL_RESULT_TOTAL_CHARS",
            self.tool_result_total_chars,
        )?;
        check_bound("JEEVES_TOOL_RESULT_MAX_CHARS", self.tool_result_max_chars)?;
        check_bound("JEEVES_TURN_TOKEN_BUDGET", self.turn_token_budget)?;
        check_bound("JEEVES_AUTO_CONTINUE_MAX", self.auto_continue_max as i64)?;
        check_bound("JEEVES_TURN_STEP_SLICE", self.turn_step_slice as i64)?;
        check_bound(
            "JEEVES_TOOL_RESULT_KEEP_RECENT",
            self.tool_result_keep_recent as i64,
        )?;
        check_bound("JEEVES_SKILL_INDEX_TOKENS", self.skill_index_tokens as i64)?;
        check_bound("JEEVES_SKILL_BODY_TOKENS", self.skill_body_tokens as i64)?;
        check_bound("JEEVES_CONTEXT_WARN_RATIO", self.context_warn_ratio as i64)?;
        if !matches!(self.approval.as_str(), "never" | "on-request") {
            return Err(format!(
                "JEEVES_APPROVAL must be never or on-request, got '{}'",
                self.approval
            ));
        }
        if !matches!(self.soul_mode.as_str(), "auto" | "thin" | "full") {
            return Err(format!(
                "JEEVES_SOUL_MODE must be auto / thin / full, got '{}'",
                self.soul_mode
            ));
        }
        if !matches!(self.tool_policy.as_str(), "eager" | "deferred") {
            return Err(format!(
                "JEEVES_TOOL_POLICY must be eager / deferred, got '{}'",
                self.tool_policy
            ));
        }

        if let Some(window) = self.context_window {
            if !(1_024..=10_000_000).contains(&window) {
                return Err(format!(
                    "JEEVES_CONTEXT_WINDOW must be between 1024 and 10000000, got {window}"
                ));
            }
            if self.max_tokens as u32 >= window {
                return Err(format!(
                    "JEEVES_MAX_TOKENS ({}) must be smaller than JEEVES_CONTEXT_WINDOW ({window}) — otherwise even the answer will not fit",
                    self.max_tokens
                ));
            }
        }

        if !(0.05..=0.95).contains(&self.compact_threshold) {
            return Err(format!(
                "JEEVES_COMPACT_THRESHOLD must be between 0.05 and 0.95, got {}",
                self.compact_threshold
            ));
        }

        for root in &self.exec_writable_roots {
            if !root.is_absolute() {
                return Err(format!(
                    "an entry in JEEVES_EXEC_WRITABLE_ROOTS must be an absolute path, got {}",
                    root.display()
                ));
            }
            if !root.is_dir() {
                return Err(format!(
                    "a directory in JEEVES_EXEC_WRITABLE_ROOTS does not exist: {}",
                    root.display()
                ));
            }
        }

        for dir in &self.write_allow_dirs {
            if !dir.is_absolute() {
                return Err(format!(
                    "an entry in JEEVES_WRITE_ALLOW_DIRS must be an absolute path, got {}",
                    dir.display()
                ));
            }
        }

        check_exec_rules(&self.exec_allow)?;
        Ok(())
    }

    /// The `config/read` payload: the readback must be the full current state; fields are total.
    pub fn view(&self) -> SettingsView {
        SettingsView {
            provider: self.provider.clone(),
            model: self.model.clone().unwrap_or_default(),
            small_model: self.small_model.clone().unwrap_or_default(),
            max_iterations: self.max_iterations,
            max_tokens: self.max_tokens,
            history_turns: self.history_turns,
            consolidate_every: self.consolidate_every,
            retrieval_top_k: self.retrieval_top_k,
            home: self.home.display().to_string(),
        }
    }
}

/// `<home>/settings.json` — where `config/write` persists.
pub fn patch_path(home: &std::path::Path) -> std::path::PathBuf {
    home.join("settings.json")
}

/// **Numeric** validation for `config/write` patches: shares `BOUNDS` with startup.
///
/// Provider existence is not checked here — `jeeves-config` does not know `PROVIDERS`
/// (that is the provider layer's business); the caller (app-server) adds that step.
pub fn validate_patch_values(patch: &jeeves_protocol::SettingsPatch) -> Result<(), String> {
    for (key, value) in [
        ("maxIterations", patch.max_iterations),
        ("maxTokens", patch.max_tokens),
        ("historyTurns", patch.history_turns),
        ("consolidateEvery", patch.consolidate_every),
        ("retrievalTopK", patch.retrieval_top_k),
    ] {
        if let Some(v) = value {
            check_bound(key, v as i64)?;
        }
    }
    if let Some(rules) = &patch.exec_allow {
        check_exec_rules(rules)?;
    }
    if let Some(dirs) = &patch.write_allow_dirs {
        if dirs.len() > MAX_EXEC_RULES {
            return Err(format!(
                "writeAllowDirs takes at most {MAX_EXEC_RULES} entries, got {}",
                dirs.len()
            ));
        }
        for dir in dirs {
            if dir.trim().is_empty() {
                return Err(
                    "writeAllowDirs has an empty path — an empty string is not an entry"
                        .to_string(),
                );
            }
            if dir.contains(['\n', '\r']) {
                return Err(format!(
                    "a writeAllowDirs path must not contain a newline: {dir:?}"
                ));
            }
            if !std::path::Path::new(dir).is_absolute() {
                return Err(format!(
                    "an entry in writeAllowDirs must be an absolute path, got {dir:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Allow rules: an empty list is legal (= nothing allowed; that is the default). But every
/// entry must be a **working** rule — a broken rule silently never matches, yet still shows
/// up in rejection reasons, which is painful to debug. Shared by startup and `config/write`.
fn check_exec_rules(rules: &[String]) -> Result<(), String> {
    if rules.len() > MAX_EXEC_RULES {
        return Err(format!(
            "JEEVES_EXEC_ALLOW takes at most {MAX_EXEC_RULES} rules, got {}",
            rules.len()
        ));
    }
    for rule in rules {
        let rule = rule.trim();
        if rule.is_empty() {
            return Err(
                "JEEVES_EXEC_ALLOW has an empty rule — an empty string is not a rule".to_string(),
            );
        }
        if rule.contains(['\n', '\r']) {
            return Err(format!(
                "a JEEVES_EXEC_ALLOW rule must not contain a newline: {rule:?}"
            ));
        }
        if rule.chars().count() > MAX_EXEC_RULE_CHARS {
            return Err(format!(
                "a JEEVES_EXEC_ALLOW rule is at most {MAX_EXEC_RULE_CHARS} characters: {rule:?}"
            ));
        }
    }
    Ok(())
}

/// Read the saved patch. A missing file or a parse failure both count as "no patch":
/// a broken config file must not keep Jeeves from starting — env vars still backstop.
pub fn load_patch(home: &std::path::Path) -> jeeves_protocol::SettingsPatch {
    read_patch(home).unwrap_or_default()
}

pub fn read_patch(home: &std::path::Path) -> std::io::Result<jeeves_protocol::SettingsPatch> {
    json_file::read(&patch_path(home))
}

/// Write the patch to `<home>/settings.json`. An all-empty patch has nothing to say — delete
/// the file, so after `config/write` resets all fields, "no overrides in this dir" holds.
pub fn save_patch(
    home: &std::path::Path,
    patch: &jeeves_protocol::SettingsPatch,
) -> std::io::Result<()> {
    let path = patch_path(home);
    if patch.is_empty() {
        match std::fs::remove_file(&path) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    json_file::write(&path, patch)
}

/// Overlay the patch onto settings. Only `Some` fields in the patch get a say.
///
/// **Models and keys are not in this patch**: they belong to `<home>/models.json`
/// (see models.rs); change them via `model/*` — one path beats two, and leaves no gap for
/// "keys casually written into settings.json".
pub fn apply_patch(patch: &jeeves_protocol::SettingsPatch, settings: &mut Settings) {
    let p = patch;
    if let Some(v) = p.max_iterations {
        settings.max_iterations = v;
    }
    if let Some(v) = p.max_tokens {
        settings.max_tokens = v;
    }
    if let Some(v) = p.history_turns {
        settings.history_turns = v;
    }
    if let Some(v) = p.consolidate_every {
        settings.consolidate_every = v;
    }
    if let Some(v) = p.retrieval_top_k {
        settings.retrieval_top_k = v;
    }
    if let Some(v) = &p.exec_allow {
        settings.exec_allow = v.clone();
    }
    if let Some(v) = &p.write_allow_dirs {
        settings.write_allow_dirs = v.iter().map(std::path::PathBuf::from).collect();
    }
    if let Some(v) = p.exec_enabled {
        settings.exec_enabled = v;
    }
    if let Some(v) = p.write_enabled {
        settings.write_enabled = v;
    }
    if let Some(v) = p.delegate_enabled {
        settings.delegate_enabled = v;
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    /// The capability switches ride the same patch as every other setting: absent leaves the
    /// environment's value alone, present overrides it, and merging keeps the older ones.
    #[test]
    fn a_patch_can_switch_capabilities_on_and_off() {
        let mut settings = Settings::default();
        assert!(!settings.exec_enabled);
        let on = jeeves_protocol::SettingsPatch {
            exec_enabled: Some(true),
            write_enabled: Some(true),
            ..Default::default()
        };
        apply_patch(&on, &mut settings);
        assert!(settings.exec_enabled && settings.write_enabled && !settings.delegate_enabled);
        let mut saved = on.clone();
        saved.merge_newer(&jeeves_protocol::SettingsPatch {
            exec_enabled: Some(false),
            ..Default::default()
        });
        apply_patch(&saved, &mut settings);
        assert!(!settings.exec_enabled && settings.write_enabled);
        assert!(!saved.is_empty());
    }

    #[test]
    fn defaults_match_the_documented_values() {
        let s = Settings::default();
        assert_eq!(s.provider, "anthropic");
        assert_eq!(s.max_iterations, 32);
        assert_eq!(s.max_tokens, 16_384);
        assert_eq!(s.history_turns, 12);
        assert_eq!(s.consolidate_every, 6);
        assert_eq!(s.retrieval_top_k, 4);
        assert_eq!(s.resume_max_age_minutes, 60);
        assert_eq!(s.home, PathBuf::from(".jeeves"));
    }

    #[test]
    fn reads_jeeves_provider_from_env() {
        std::env::set_var("JEEVES_PROVIDER", "deepseek");
        let s = Settings::from_env();
        assert_eq!(
            s.provider, "deepseek",
            "JEEVES_PROVIDER should have been read"
        );
        std::env::remove_var("JEEVES_PROVIDER");
    }

    #[test]
    fn empty_env_value_is_treated_as_unset() {
        std::env::set_var("JEEVES_MODEL", "");
        let s = Settings::from_env();
        assert_eq!(s.model, None);
        std::env::remove_var("JEEVES_MODEL");
    }

    #[test]
    fn bool_accepts_the_three_spellings() {
        for (v, expected) in [
            ("1", true),
            ("true", true),
            ("yes", true),
            ("0", false),
            ("", false),
        ] {
            let key = "JEEVES_CONFIG_TEST_BOOL_XYZ";
            std::env::set_var(key, v);
            assert_eq!(env_bool(key), expected, "{key}={v}");
            std::env::remove_var(key);
        }
    }

    #[test]
    fn view_is_complete() {
        let s = Settings::default();
        let v = s.view();
        assert_eq!(v.provider, "anthropic");
        assert_eq!(v.max_tokens, 16_384);
    }

    #[test]
    fn apply_patch_overlays_only_the_fields_it_carries() {
        let mut s = Settings::default();
        let patch = jeeves_protocol::SettingsPatch {
            max_tokens: Some(4096),
            ..Default::default()
        };
        apply_patch(&patch, &mut s);
        assert_eq!(s.max_tokens, 4096);
        assert_eq!(s.max_iterations, Settings::default().max_iterations);
    }

    /// Models and keys are **not** in this patch — change them via `model/*`; keys go only into
    /// models.json / the keychain. This assertion pins "the patch has no such gaps".
    #[test]
    fn the_patch_has_no_model_or_key_fields_to_leak() {
        let patch = jeeves_protocol::SettingsPatch::default();
        let serialized = serde_json::to_string(&patch).expect("serialise");
        for leaked in ["provider", "model", "smallModel", "apiKey", "baseUrl"] {
            assert!(
                !serialized.contains(leaked),
                "a patch must not carry model or key fields: {serialized}"
            );
        }
    }

    #[test]
    fn merge_newer_keeps_newer_values_and_keeps_older_ones_otherwise() {
        let mut base = jeeves_protocol::SettingsPatch {
            max_iterations: Some(5),
            max_tokens: Some(1024),
            ..Default::default()
        };
        let newer = jeeves_protocol::SettingsPatch {
            max_iterations: Some(9),
            ..Default::default()
        };
        base.merge_newer(&newer);
        assert_eq!(base.max_iterations, Some(9));
        assert_eq!(
            base.max_tokens,
            Some(1024),
            "fields the new patch does not mention must survive"
        );
    }

    #[test]
    fn patch_round_trips_through_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let patch = jeeves_protocol::SettingsPatch {
            max_iterations: Some(20),
            ..Default::default()
        };
        save_patch(dir.path(), &patch).expect("write to disk");
        let loaded = load_patch(dir.path());
        assert_eq!(loaded.max_iterations, Some(20));
    }

    #[test]
    fn saving_an_empty_patch_removes_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        save_patch(
            dir.path(),
            &jeeves_protocol::SettingsPatch {
                max_iterations: Some(20),
                ..Default::default()
            },
        )
        .expect("write one first");
        save_patch(dir.path(), &jeeves_protocol::SettingsPatch::default()).expect("then clear it");
        assert!(
            !patch_path(dir.path()).exists(),
            "an empty patch must not leave a file"
        );
    }

    #[test]
    fn a_corrupt_patch_file_reads_as_no_patch() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(patch_path(dir.path()), "this is not JSON").expect("write the broken file");
        assert!(load_patch(dir.path()).is_empty());
        assert!(load_patch(std::path::Path::new("/definitely/not/here")).is_empty());
    }

    #[test]
    fn the_defaults_pass_validation() {
        assert!(Settings::default().validate().is_ok());
    }

    #[test]
    fn an_out_of_range_number_names_the_env_var() {
        let s = Settings {
            history_turns: -5,
            ..Settings::default()
        };
        let error = s.validate().expect_err("a negative window must be caught");
        assert!(error.contains("JEEVES_HISTORY_TURNS"), "{error}");
        assert!(
            error.contains("-5"),
            "the value received must be stated: {error}"
        );

        let s = Settings {
            exec_timeout_secs: 0,
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("a zero-second timeout means nothing")
            .contains("JEEVES_EXEC_TIMEOUT"));

        let s = Settings {
            max_tokens: 4,
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("4 tokens cannot answer anything")
            .contains("JEEVES_MAX_TOKENS"));
    }

    #[test]
    fn broken_exec_allow_rules_are_caught_at_startup() {
        let s = Settings {
            exec_allow: vec!["cargo test".to_string(), "   ".to_string()],
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("an empty rule must be caught")
            .contains("empty rule"));

        let s = Settings {
            exec_allow: vec!["cargo test\nrm -rf /".to_string()],
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("a rule with a newline must be caught")
            .contains("newline"));

        let s = Settings {
            exec_allow: vec!["x".repeat(300)],
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("an over-long rule must be caught")
            .contains("at most"));

        let s = Settings {
            exec_allow: vec!["ls".to_string(); 100],
            ..Settings::default()
        };
        assert!(s
            .validate()
            .expect_err("too many rules must be caught")
            .contains("at most"));

        let s = Settings {
            exec_allow: Vec::new(),
            ..Settings::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn the_compaction_threshold_is_a_ratio() {
        for bad in [0.0, 0.01, 1.0, 2.0, -0.5] {
            let s = Settings {
                compact_threshold: bad,
                ..Settings::default()
            };
            let error = s
                .validate()
                .expect_err("a ratio out of range must be caught");
            assert!(error.contains("JEEVES_COMPACT_THRESHOLD"), "{error}");
        }
        let s = Settings {
            compact_threshold: 0.5,
            ..Settings::default()
        };
        assert!(s.validate().is_ok());
    }

    #[test]
    fn a_window_override_must_be_sane() {
        let too_small = Settings {
            context_window: Some(10),
            ..Settings::default()
        };
        assert!(too_small
            .validate()
            .expect_err("a window of 10 means nothing")
            .contains("JEEVES_CONTEXT_WINDOW"));

        let impossible = Settings {
            context_window: Some(4096),
            max_tokens: 8192,
            ..Settings::default()
        };
        assert!(impossible
            .validate()
            .expect_err("the answer does not fit")
            .contains("JEEVES_MAX_TOKENS"));

        let ok = Settings {
            context_window: Some(8192),
            max_tokens: 1024,
            ..Settings::default()
        };
        assert!(ok.validate().is_ok());
    }

    #[test]
    fn writable_roots_must_exist_and_be_absolute() {
        let dir = tempfile::tempdir().expect("temp dir");
        let ok = Settings {
            exec_writable_roots: vec![dir.path().to_path_buf()],
            ..Settings::default()
        };
        assert!(ok.validate().is_ok());

        let missing = Settings {
            exec_writable_roots: vec![std::env::temp_dir().join("jeeves-not-here-9f3a1")],
            ..Settings::default()
        };
        let why = missing
            .validate()
            .expect_err("a directory that does not exist must be caught");
        assert!(
            why.contains("does not exist"),
            "this must land on the \"does not exist\" branch: {why}"
        );

        let relative = Settings {
            exec_writable_roots: vec![PathBuf::from("build")],
            ..Settings::default()
        };
        assert!(relative
            .validate()
            .expect_err("a relative path cannot go into a sandbox rule")
            .contains("absolute path"));
    }

    #[test]
    fn the_sandbox_is_offline_unless_asked_otherwise() {
        assert!(!Settings::default().exec_network);
        assert!(Settings::default().exec_writable_roots.is_empty());
    }

    #[test]
    fn patch_validation_reuses_the_same_bounds() {
        use jeeves_protocol::SettingsPatch;
        let bad = SettingsPatch {
            max_tokens: Some(1),
            ..Default::default()
        };
        let error = validate_patch_values(&bad).expect_err("an out-of-range patch must be refused");
        assert!(error.contains("maxTokens"), "{error}");
        assert!(
            error.contains("128"),
            "the lower bound must be stated: {error}"
        );

        let ok = SettingsPatch {
            max_tokens: Some(4096),
            history_turns: Some(0),
            ..Default::default()
        };
        assert!(validate_patch_values(&ok).is_ok());
    }
}
