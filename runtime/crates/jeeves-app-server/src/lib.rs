//! `jeeves-app-server` — the JSON-RPC server, the process where Jeeves's brain lives.
//!
//! The fulcrum of the whole three-language architecture: **only this process opens state.db**.
//! The CLI, dashboard, chat gateway, and Python SDK are all its clients, speaking newline-delimited
//! JSON-RPC over stdio. Therefore:
//!
//!   * the problem of sharing a SQLite connection across threads does not exist (no sharing needed at all);
//!   * async exists in exactly one place, the server (the gateway can stay TypeScript, using the existing ecosystem);
//!   * state is single: clients in any language all see the same memory.
//!
//! P2b added streaming: every time the model emits a chunk of text, the client immediately receives a `textDelta`.
//! All outbound output (notifications and responses) converges into one channel, written to stdout by the
//! single writer — order is thereby guaranteed, and no concurrent write can corrupt a line.
//!
//! It can start even without a configured key: memory and session methods work as usual, only `turn/*`
//! reports PROVIDER_ERROR, telling the caller where to get a key.

mod agents_md;
mod approval;
mod approval_push;
mod browser;
mod capability;
mod command_check;
mod completion_delivery;
mod connector_oauth;
mod connector_settings;
mod connector_tools;
mod connector_wait;
mod desktop;
mod devices;
mod dispatch;
mod edit_check_host;
mod goal;
mod image_meta;
mod input_wake;
mod interaction;
pub use interaction::{MAIN_SESSION, main_session, submit_main};
mod job_delivery;
mod jobs;
mod ledger_writer;
/// Guardian lock: only one app-server per home (`jeeves_app_server::lock`).
pub mod lock;
mod mcp_renewal;
pub mod memory_center;
#[cfg(test)]
mod memory_center_tests;
mod memory_worker;
pub mod models;
mod pending_approvals;
mod permissions;
#[cfg(test)]
mod permissions_tests;
mod resources;
mod responsibility;
mod resume;
mod resume_task;
mod route;
mod schedule_output;
mod scheduler;
mod server;
mod stdio;
mod subagent;
mod task;
mod trace;
mod turn;
mod wake;

#[cfg(test)]
#[path = "resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "approval_tests.rs"]
mod approval_tests;

#[cfg(test)]
#[path = "todo_tests.rs"]
mod todo_tests;

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod jobs_tests;

#[cfg(test)]
#[path = "lock_tests.rs"]
mod lock_tests;

#[cfg(test)]
#[path = "goal_tests.rs"]
mod goal_tests;

#[cfg(test)]
#[path = "image_meta_tests.rs"]
mod image_meta_tests;

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod subagent_tests;

use jeeves_config::Settings;
use jeeves_loop::{Interrupt, Steering};
use jeeves_provider::Resolved;
use jeeves_state::{Chat, Episodes, Facts};
use jeeves_tools::ToolRegistry;
use sqlx::sqlite::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Mutex as AsyncMutex;

pub use dispatch::handle;
pub use scheduler::{acknowledge as acknowledge_schedule, attach_scheduler};
pub use server::events::{EventSink, Frame};
pub use server::open;
pub(crate) use server::tools::{BuiltinTools, builtin_tools};
pub(crate) use server::tools::{Injections, enabled_tools, tool_ctx, tool_result_budget};
pub use stdio::run_stdio;
pub use turn::run_turn;

/// Outcome of `turn/follow-up`: queued (with its position), nothing to wait for, or a storage
/// failure. Three different things to tell a person; one enum instead of a bare `Result`.
pub(crate) enum FollowUp {
    Queued(i32),
    Idle,
    Failed(String),
}

/// A turn that is in flight: the cancellation token plus the two facts `turn/list` answers with.
/// One record instead of a second side table — two maps keyed by turn_id would eventually disagree.
pub(crate) struct ActiveTurn {
    pub(crate) interrupt: Arc<Interrupt>,
    pub(crate) session_id: String,
    /// RFC 3339 with offset, captured at registration.
    pub(crate) started_at: String,
    /// Sentences added while this turn runs (`turn/steer`). The loop folds them in at the next
    /// iteration boundary; the transcript reads them all afterwards.
    pub(crate) steering: Arc<Mutex<Steering>>,
}

/// Everything the server holds.
///
/// `resolved` is an `Option`: it must be able to start without a configured key. Memory and sessions
/// do not depend on the model; only `turn/*` needs it — and that is when "missing key" gets spelled out.
///
/// `settings` / `resolved` sit in `RwLock`s: `config/write` changes them while running, and the next
/// turn after the change uses the new values — no process restart needed. The lock only guards the few
/// microseconds of read-modify-write; there is no await inside.
///
/// `turns` is the cancellation-token table of running turns: `turn/interrupt` finds one by turn_id
/// and flips the switch. A finished turn (success or failure) is removed. Each entry also carries
/// the session it belongs to, because `turn/list` is asked **by session** — a table keyed only by
/// turn_id cannot answer "is this conversation running right now".
///
/// `Clone`: every field is a handle (Arc / connection pool / config); copying one does not copy state.
/// The full_agent node in the graph is a 'static closure and can only hold what it owns —
/// what it holds is exactly this clone.
#[derive(Clone)]
pub struct Server {
    pub(crate) browser: Arc<browser::BrowserHost>,
    pub(crate) permissions: Arc<permissions::Policies>,
    pub(crate) facts: Facts,
    pub(crate) episodes: Episodes,
    pub(crate) chat: Chat,
    /// The append-only turn ledger (migration 0020): the record `chat_log` is a view of.
    /// Written at finalize time; read back by the memory worker and the memory center.
    pub(crate) ledger: jeeves_state::Ledger,
    /// The background memory worker (`open()` attaches it; tests and embedders may not).
    /// Its absence is not a degradation: `finalize_turn` falls back to doing the same
    /// work inline, so a library user who never starts a runtime still gets memory.
    pub(crate) memory_worker: Option<Arc<memory_worker::MemoryWorker>>,
    /// Usage ledger (migration 0010): bookkeeping only, never enforced.
    pub(crate) usage: jeeves_state::Usage,
    /// Turn checkpoints (migration 0013): a running turn's working memory, durable at each
    /// tool-results boundary — the fresh process resumes instead of replaying finished tools.
    pub(crate) checkpoints: jeeves_state::Checkpoints,
    /// Notifier (M3a): a side path; None when unconfigured.
    pub(crate) notifier: Option<Arc<dyn jeeves_tools::notify::Notifier>>,
    /// Approval audit ledger (M3b): who asked, and how it turned out.
    pub(crate) approval_log: jeeves_state::Approvals,
    pub(crate) settings: Arc<RwLock<Settings>>,
    pub(crate) boot_settings: Arc<Settings>,
    pub(crate) resolved: Arc<RwLock<Option<Resolved>>>,
    pub(crate) tools: Arc<ToolRegistry>,
    pub(crate) turns: Arc<Mutex<HashMap<String, ActiveTurn>>>,
    /// Approval requests waiting for a human: `approval/respond` finds one by turn_id + request_id
    /// and delivers the answer back. Same lifecycle discipline as `turns` (the whole bucket is dropped when a turn ends).
    pub(crate) approvals: approval::Pending,
    /// The live allow table (`execAllow`): the cell the exec policy reads, replaced whole by
    /// `config/write`, so a revoked rule stops matching on the next call.
    pub(crate) exec_allow: Arc<Mutex<Vec<String>>>,
    /// Commands a human answered "always allow in this session" to, live for this process: the very same cell the
    /// exec tool reads (`ExecPolicy::session_allow`), so the next call sees the yes immediately —
    /// not after a restart. Never persisted (that is `settings.json`'s `execAllow`).
    pub(crate) exec_session_allow: Arc<Mutex<Vec<String>>>,
    /// Directories a write was approved for ("always allow in this session" / "remember this one"), same idea: the cell is
    /// shared with `WritePolicy::approved_dirs`, so the next write skips the question. The
    /// persisted half comes from `settings.json`'s `writeAllowDirs` at startup.
    pub(crate) write_approved_dirs: Arc<Mutex<Vec<PathBuf>>>,
    /// MCP connections; ended politely (HTTP `DELETE`) when the stdio loop finishes.
    pub(crate) mcp: Arc<server::mcp_live::McpLive>,
    /// Session -> todo list (see todo.rs in jeeves-tools). Injected into the system prompt every turn,
    /// so it does not depend on history and compaction cannot wash it away.
    pub(crate) todo: Arc<jeeves_tools::todo::TodoBoard>,
    /// Background job table (switched on/off together with `JEEVES_EXEC`).
    pub(crate) jobs: Option<Arc<dyn jeeves_tools::jobs::JobRegistry>>,
    /// Long tasks (migration 0014, rebuilt in 0017): identity, status, acceptance, budget,
    /// spent. Human-only switches live in task.rs; the goal loop is the engine that drives rounds.
    pub(crate) tasks: jeeves_state::Tasks,
    pub(crate) completions: Arc<jeeves_state::completions::Completions>,
    pub(crate) completion_lock: Arc<AsyncMutex<()>>,
    /// One row per execution of a task (migration 0017): "executed N times", what each cost.
    pub(crate) runs: jeeves_state::Runs,
    /// One row per tool call (migration 0017) — where idempotency lives: a step left `running`
    /// is never replayed, it waits for a human.
    pub(crate) steps: jeeves_state::Steps,
    /// The task timeline: verification rounds, budget refusals, contradictions.
    pub(crate) events: jeeves_state::Events,
    /// The deliverable list: files that really exist after a round.
    pub(crate) artifacts: jeeves_state::Artifacts,
    /// Messages queued to run **after** the turn in flight (0.16 follow-up, migration 0019).
    pub(crate) queue: jeeves_state::Queue,
    /// Turns a human just asked to suspend (`task/suspend`): at finalize time the checkpoint is
    /// kept as `suspended` instead of `abandoned` — pausing is not stopping.
    pub(crate) suspending: Arc<Mutex<HashSet<String>>>,
    /// Multiple model profiles (`<home>/models.json`) and the credentials backend (see models.rs).
    /// Switching models = edit here + re-resolve the provider, no restart.
    pub(crate) models: models::Models,
    pub(crate) secrets: models::Secrets,
    /// Models resolved per tier (route.rs): each profile is resolved once — the provider
    /// client holds the connection pool. When a profile is edited/deleted, `reapply` invalidates the whole table.
    pub(crate) route_cache: Arc<std::sync::Mutex<std::collections::HashMap<String, Resolved>>>,
    /// The merged model catalog (built-in table + `<home>/models-catalog.json`), loaded
    /// once at boot. `model/save` validates against it; `model/list` shows it.
    pub(crate) catalog: Arc<jeeves_provider::catalog::Catalog>,
    /// One **write-section lock** per session: the serialisation point for a turn's
    /// write sections (history append / consolidation / compaction). Model-call
    /// sections never hold it — that would stall the session and keep interrupt out.
    /// Lazily created and **never removed**: removal would race two concurrent turns
    /// into holding two different locks.
    pub(crate) session_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
    _execution_owner: Option<Arc<lock::DaemonLock>>,
    pub(crate) pool: SqlitePool,
    /// The last turn's context breakdown per session — the memory center's meter.
    ///
    /// Written at assembly time (that is the only moment every section is in hand), read by
    /// `context/usage`. Absent = no turn has run yet, which the meter shows as "not measured"
    /// rather than a fake zero.
    pub(crate) context_usage: Arc<Mutex<HashMap<String, jeeves_protocol::ContextUsage>>>,
    /// Deferred-tool activations, per session (`JEEVES_TOOL_POLICY=deferred`).
    ///
    /// A session-scoped cell, not a per-turn one: `find_tools` runs in one turn and its
    /// result is read in the next, so a fresh set every turn would make the model rediscover
    /// the same tool on every message. In memory on purpose — a restarted server asks the
    /// model to search again, which is honest and cheap; a stale table on disk is neither.
    pub(crate) activations: Arc<Mutex<HashMap<String, jeeves_tools::ActivationSet>>>,
    /// Capability cards, cached in memory for 60 seconds each.
    pub(crate) capabilities: capability::Cards,
    /// Action grants and durable pending approvals (migration 0003).
    pub(crate) grants: jeeves_state::Grants,
    /// Questions that outlived a restart, waiting for a human answer (version already bumped).
    pub(crate) restored_approvals: Arc<Mutex<Vec<jeeves_state::grants::PendingRow>>>,
    /// One-time yeses answered to restored questions: `tool\0args`. The resumed turn's re-ask
    /// consumes one instead of asking again.
    pub(crate) resume_yes: Arc<Mutex<std::collections::HashSet<String>>>,
}

#[cfg(test)]
mod completion_tests;

#[cfg(test)]
mod input_wake_tests;

#[cfg(test)]
mod task_wake_tests;
#[cfg(test)]
mod task_worktree_tests;

#[cfg(test)]
mod input_control_tests;

#[cfg(test)]
mod runtime_ownership_tests;

#[cfg(test)]
mod recovery_usage_tests;

#[cfg(test)]
mod memory_recovery_tests;

pub(crate) fn narrow(value: i64) -> i32 {
    value.clamp(0, i32::MAX as i64) as i32
}

#[cfg(test)]
mod pending_approval_tests;

#[cfg(test)]
mod device_tests;

#[cfg(test)]
mod desktop_tests;
