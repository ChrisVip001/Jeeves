//! Turn lifecycle facade; stages share the same fresh/recovered execution contracts.

pub(crate) mod driver;
mod progress;

mod entry;
pub use entry::run_turn;
pub(crate) use entry::run_turn_inner;
mod finalize;
pub(crate) use finalize::FinishedTurn;
pub(crate) use finalize::finalize_turn;
mod settlement;
use settlement::Settlement;
use settlement::settle_task;
mod lifecycle;
pub(crate) use lifecycle::TurnGuard;
pub(crate) use lifecycle::checkpoint_outlet;
pub(crate) use lifecycle::drain_queue;
pub(crate) use lifecycle::fail_turn;
pub(crate) use lifecycle::new_turn_id;
use lifecycle::open_run;
use lifecycle::retry_outlet;
mod accounting;
pub(crate) mod meter;
pub(crate) use accounting::RoundCtx;
pub(crate) use accounting::TaskDecision;
pub(crate) use accounting::account_round;
pub(crate) use accounting::{budget_overrun, budget_remaining};
pub(crate) use accounting::{observe_round, restore_observation};
mod context;
use context::PRELOOP_TIMEOUT;
pub(crate) use context::TurnFrame;
pub(crate) use context::interruptible;
pub(crate) use context::resolve_turn_cwd;
use context::retrieve_memory;
pub(crate) use context::session_cwd;
mod execution;
pub(crate) use execution::FullTurn;
pub(crate) use execution::full_turn;
mod prompt;
mod schedule_lookup;
pub(crate) use prompt::PromptParts;

pub(crate) use prompt::build_system;
pub(crate) use prompt::capability_note;
pub(crate) use prompt::load_soul;
pub(crate) use prompt::thin_soul_for;
mod attachments;
mod capability;
use attachments::resolve_attachments;
mod history;
mod title;
use history::FORCED_RETRY_TURNS;
pub(crate) use history::HistoryStats;
pub(crate) use history::Loaded;
use history::SUMMARY_RESERVE_TOKENS;
use history::budget_tokens;
pub(crate) use history::fold_tool_activity;
use history::is_context_overflow;
use history::load_history;
pub(crate) use history::strip_echoed_footer;

mod plan;
use plan::load_plan;

pub(crate) mod failure;
pub(crate) mod operations;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod operations_tests;
