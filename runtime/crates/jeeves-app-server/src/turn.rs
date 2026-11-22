//! Turn lifecycle facade; stages share the same fresh/recovered execution contracts.

pub(crate) mod driver;

mod entry;
pub use entry::run_turn;
pub(crate) use entry::run_turn_inner;
mod finalize;
pub(crate) use finalize::finalize_turn;
pub(crate) use finalize::FinishedTurn;
mod settlement;
use settlement::settle_task;
use settlement::Settlement;
mod lifecycle;
pub(crate) use lifecycle::checkpoint_outlet;
pub(crate) use lifecycle::drain_queue;
pub(crate) use lifecycle::fail_turn;
use lifecycle::new_turn_id;
use lifecycle::open_run;
use lifecycle::retry_outlet;
pub(crate) use lifecycle::TurnGuard;
mod accounting;
pub(crate) use accounting::account_round;
pub(crate) use accounting::observe_round;
pub(crate) use accounting::RoundCtx;
pub(crate) use accounting::TaskDecision;
pub(crate) use accounting::{budget_overrun, budget_remaining};
mod context;
pub(crate) use context::interruptible;
pub(crate) use context::resolve_turn_cwd;
use context::retrieve_memory;
pub(crate) use context::session_cwd;
pub(crate) use context::TurnFrame;
use context::PRELOOP_TIMEOUT;
mod execution;
pub(crate) use execution::full_turn;
pub(crate) use execution::FullTurn;
mod prompt;
pub(crate) use prompt::PromptParts;

pub(crate) use prompt::build_system;
pub(crate) use prompt::capability_note;
pub(crate) use prompt::load_soul;
pub(crate) use prompt::thin_soul_for;
mod attachments;
use attachments::resolve_attachments;
mod history;
use history::budget_tokens;
pub(crate) use history::fold_tool_activity;
use history::is_context_overflow;
use history::load_history;
pub(crate) use history::HistoryStats;
pub(crate) use history::Loaded;
use history::FORCED_RETRY_TURNS;
use history::SUMMARY_RESERVE_TOKENS;

mod plan;
use plan::load_plan;

pub(crate) mod failure;
pub(crate) mod operations;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod operations_tests;
