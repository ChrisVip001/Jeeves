//! `jeeves-state` — everything Jeeves remembers lives in this one SQLite file.
//!
//! The app-server runtime owns database access; browser, gateway, and SDK clients use
//! JSON-RPC. Embedded runtimes can still be started in separate processes (CLI, scheduler,
//! or MCP), so SQLite WAL and connection-level busy timeouts remain necessary. Production execution startup takes a shared OS ownership lock before recovery.
//! Memory-only state users still rely on SQLite transactions; this crate does not acquire that lock.
//!
//! Layers:
//!   * `db`        open / migrate
//!   * `ledger`    the append-only record of every turn (the rebuild source)
//!   * `facts`     semantic memory (FTS5 retrieval)
//!   * `episodes`  episodic memory
//!   * `chat`      the conversation *view* + sessions (a session is just a tag on chat_log)
//!   * `fts`       turn human input into a safe FTS5 query string

pub mod approvals;
pub mod artifacts;
pub mod chat;
pub mod checkpoints;
pub mod completions;
pub mod conformance;
pub mod db;
pub mod devices;
pub mod dispatch;
pub mod edit_checks;
pub mod episodes;
pub mod events;
pub mod external_waits;
pub mod facts;
pub mod fts;
pub mod git_checkpoints;
pub mod grants;
pub mod human_pauses;
pub mod interactions;
pub mod jobs;
pub mod ledger;
pub mod mail_actions;
pub mod operations;
pub mod plans;
pub mod queue;
pub mod receipts;
pub mod responsibilities;
pub mod runs;
pub mod schedule_completions;
pub mod schedule_jobs;
pub mod schedules;
pub mod session_permissions;
pub mod steps;
pub mod store;
pub mod task_sources;
pub mod tasks;
pub mod usage;
pub mod verification;
pub mod waits;
pub mod wake_inbox;
pub mod weekly_reports;

pub use approvals::{ApprovalLogRow, Approvals};
pub use artifacts::{ArtifactRow, Artifacts};
pub use chat::{Chat, MessageRow, SessionRow};
pub use checkpoints::{CheckpointRow, Checkpoints};
pub use db::open;
pub use edit_checks::EditChecks;
pub use episodes::{EpisodeRow, Episodes};
pub use events::{EventRow, Events};
pub use facts::{FactMeta, FactRow, Facts};
pub use grants::{grant_covers, Grant, GrantRequest, Grants};
pub use jobs::{JobRow, Jobs};
pub use ledger::BackfillReport;
pub use ledger::{EntryRow, Ledger, NewEntry, TurnRow};
pub use plans::Plans;
pub use queue::{Queue, QueuedMessage};
pub use receipts::{ReceiptRow, Receipts};
pub use runs::{RunRow, RunStatus, Runs};
pub use schedule_jobs::{ScheduleJobRow, ScheduleJobs};
pub use schedules::{ScheduleRow, Schedules};
pub use steps::{NewStep, StepRow, Steps};
pub use store::{EpisodicStore, SemanticStore};
pub use tasks::{NewTask, TaskRow, TaskStatus, Tasks};
pub use usage::{Usage, UsageByModel, UsageSummary};
pub use wake_inbox::{WakeInbox, WakeOutcome, WakeRow};

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;

#[cfg(test)]
#[path = "conformance_tests.rs"]
mod conformance_tests;

#[cfg(test)]
mod dispatch_tests;

#[cfg(test)]
mod completion_tests;

#[cfg(test)]
mod schedule_completion_tests;

#[cfg(test)]
mod checkpoint_round_tests;

#[cfg(test)]
mod memory_edit_tests;

#[cfg(test)]
mod external_wait_tests;

#[cfg(test)]
mod human_pause_tests;

#[cfg(test)]
mod operation_tests;

#[cfg(test)]
mod verification_tests;
