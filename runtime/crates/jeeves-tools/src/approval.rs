//! Interactive approval: **off by default, deny by default, silence means denial**.
//!
//! This layer has only one trait — the tool layer knows no protocol and no Server;
//! who asks and how is up to the implementer (same discipline as `subagent.rs`).
//! Implementations live in `jeeves-app-server`: it has the outlet and the wait table.
//!
//! Three rules that must not bend:
//!
//! 1. **Deny by default**. No broker (terminal not asking, cockpit not open), timeout,
//!    dropped connection, late answer — nothing passes; approval has one source: a "yes".
//! 2. **Approval only unlocks the allow table**. Hard-deny list and sandbox are non-negotiable:
//!    the gates in `vet` return before approval ever runs; approval never touches them.
//! 3. **Timeouts are the implementer's responsibility**. `request` must honor `timeout_secs`
//!    itself and return [`ApprovalOutcome::TimedOut`] — a request that never returns hangs a
//!    conversation forever.
//!
//! The call site names **what** could be remembered ([`RememberTarget`]); how long a yes lasts
//! (this once / this process / forever) is the answerer's decision, so it stays out of this layer.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

/// What a "remember this" answer should write down.
///
/// Naming it here — rather than making the answerer parse the preview text — keeps "what the user
/// approved is exactly what is remembered". Nothing in here can relax a hard gate: a remembered
/// command still meets hard-deny and the sandbox, and a remembered directory still has to be inside
/// the writable boundary (`contained` runs first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RememberTarget {
    /// An exact command line (`run_command`). No wildcards: what the human saw was this line.
    Command(String),
    /// A directory (write tools). The file name is deliberately **not** remembered — the human
    /// approved "this folder", not "this one write".
    Directory(PathBuf),
}

/// A scoped, durable authorization for one external action. The host consumes a matching grant
/// before asking again, and persists a human's answer under the same scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionGrant {
    pub account: String,
    pub resource: String,
    pub action: String,
    pub destination: String,
    pub content_hash: Option<String>,
    pub scope: String,
    pub expires_at: Option<i64>,
}

/// A single approval request.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// What to run (tool name).
    pub tool: String,
    /// Human-facing preview: the raw command, or a summary of arguments.
    pub args_preview: String,
    /// Why it wasn't let through by the allow rules directly.
    pub reason: String,
    /// Seconds to wait for a human answer. The implementer must give up after it and return `TimedOut`.
    pub timeout_secs: i64,
    /// What "remember this" would record. `None` = nothing worth remembering (ask every time).
    pub remember: Option<RememberTarget>,
    /// Contract ③: a scoped action this question authorizes. Hosts consume an existing grant
    /// before displaying a card and persist the answer for the next identical action.
    pub action: Option<ActionGrant>,
}

/// How an approval request ended. Only [`ApprovalOutcome::Approved`] lets the action run.
///
/// The refusals are kept apart because the model acts on them differently: a person saying no
/// closes the question, silence leaves it open, and a withdrawn request says nothing about what
/// the person wants. Folding them into one "denied or expired" text made models blame the
/// approval channel after a plain no.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Approved,
    /// A person answered no.
    Denied,
    /// Nobody answered before the request's deadline.
    TimedOut,
    /// The request ended without an answer: the turn ended, permissions changed, nobody can
    /// answer in this session, or the approval could not be recorded.
    Withdrawn,
}

impl ApprovalOutcome {
    pub fn approved(self) -> bool {
        self == Self::Approved
    }

    /// The text the model reads when `what` (for example "write" or "run") did not happen.
    pub fn refusal(self, what: &str) -> String {
        match self {
            Self::Approved => format!("This {what} was approved."),
            Self::Denied => format!(
                "The user denied this {what}; it did not happen. Do not ask for it again unless the user asks again."
            ),
            Self::TimedOut => format!(
                "Nobody answered the approval request for this {what} in time; it did not happen. Ask the user before trying again."
            ),
            Self::Withdrawn => format!(
                "The approval request for this {what} ended without an answer (the turn ended, permissions changed or nobody can answer here); it did not happen."
            ),
        }
    }
}

pub type ApprovalFut = Pin<Box<dyn Future<Output = ApprovalOutcome> + Send>>;

/// Who answers "should this run". The implementer handles asking, waiting, and timeouts.
pub trait ApprovalBroker: Send + Sync {
    fn request(&self, request: ApprovalRequest) -> ApprovalFut;
}
