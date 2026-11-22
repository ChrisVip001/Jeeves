//! The server side of the approval question.
//!
//! `jeeves-tools` only knows the `ApprovalBroker` trait (the tool layer knows neither protocol nor
//! Server); this is its implementation: emit one `approvalRequested` notification, register the answer
//! channel into the waiting table, then **wait**. No answer (timeout, lost connection, too late) means rejection — default-deny is the foundation.
//!
//! The waiting table is bucketed by turn_id, the same shape as `turns` (the cancellation-token table):
//! a finished turn removes the whole bucket, so there is never "last turn's question still waiting for an answer that will never come".

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jeeves_protocol::{ApprovalRequestedNotification, ServerNotification};
use jeeves_tools::approval::{
    ActionGrant, ApprovalBroker, ApprovalFut, ApprovalRequest, RememberTarget,
};
use tokio::sync::oneshot;

use crate::{narrow, EventSink};

/// One question currently waiting for a human answer.
pub(crate) struct Waiting {
    /// The answer comes back here.
    pub(crate) tx: oneshot::Sender<(bool, String)>,
    /// What "remember this one" would record, as named by the asking tool (the command, or the directory).
    ///
    /// Kept **here**, not inferred from the preview text at answer time: parsing a human-facing
    /// string to decide what gets a standing permission is how the wrong thing gets remembered.
    pub(crate) remember: Option<RememberTarget>,
    pub(crate) request: ApprovalRequestedNotification,
    started: Instant,
    timeout: Duration,
}
impl Waiting {
    pub(crate) fn new(
        tx: oneshot::Sender<(bool, String)>,
        remember: Option<RememberTarget>,
        request: ApprovalRequestedNotification,
        timeout: Duration,
    ) -> Self {
        Self {
            tx,
            remember,
            request,
            started: Instant::now(),
            timeout,
        }
    }
    pub(crate) fn snapshot(&self) -> Option<ApprovalRequestedNotification> {
        let remaining = self.remaining();
        if remaining.is_zero() {
            return None;
        }
        let mut request = self.request.clone();
        request.expires_in_ms = narrow(remaining.as_millis().min(i64::MAX as u128) as i64);
        Some(request)
    }
    pub(crate) fn remaining(&self) -> Duration {
        if self.tx.is_closed() {
            Duration::ZERO
        } else {
            self.timeout.saturating_sub(self.started.elapsed())
        }
    }
}

/// turn_id → request_id → the waiting question.
pub(crate) type Pending = Arc<Mutex<HashMap<String, HashMap<String, Waiting>>>>;

pub(crate) struct Bridge {
    pub(crate) pending: Pending,
    pub(crate) push: Option<crate::Server>,
    pub(crate) sink: EventSink,
    pub(crate) turn_id: String,
    pub(crate) timeout_secs: i64,
    /// People off-screen should also know "approval is being awaited" — a side path; absent means nothing is sent.
    pub(crate) notify: Option<Arc<dyn jeeves_tools::notify::Notifier>>,
    /// Audit ledger (M3b): all four outcome states each leave one row.
    pub(crate) logbook: jeeves_state::Approvals,
    /// Durable copy of the question, so a restart does not lose it.
    pub(crate) grants: jeeves_state::Grants,
    pub(crate) runs: jeeves_state::Runs,
    pub(crate) events: jeeves_state::Events,
    pub(crate) session_id: String,
    /// See `Server::restored_approvals`: a re-ask of the same question takes the row over.
    pub(crate) restored: Arc<Mutex<Vec<jeeves_state::grants::PendingRow>>>,
    /// See `Server::resume_yes`.
    pub(crate) resume_yes: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// Key of a one-time yes: the same tool asking about the same thing.
pub(crate) fn yes_key(turn: &str, tool: &str, args_preview: &str) -> String {
    format!("{turn}\0{tool}\0{args_preview}")
}

impl ApprovalBroker for Bridge {
    fn request(&self, request: ApprovalRequest) -> ApprovalFut {
        let request_id = format!(
            "ap-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_micros(),
            next_request_id()
        );
        let key = yes_key(&self.turn_id, &request.tool, &request.args_preview);
        let spent = self
            .resume_yes
            .lock()
            .map(|mut set| set.remove(&key))
            .unwrap_or(false);
        if spent {
            eprintln!(
                "(jeeves) approval for {} reused the answer given after the restart",
                request.tool
            );
            let grants = self.grants.clone();
            let action = request.action;
            return Box::pin(async move {
                match action {
                    Some(action) => persist_grant(&grants, action).await,
                    None => true,
                }
            });
        }
        let version = self
            .restored
            .lock()
            .ok()
            .and_then(|mut rows| {
                let at = rows.iter().position(|r| {
                    r.turn_id == self.turn_id
                        && r.tool == request.tool
                        && r.args_preview == request.args_preview
                })?;
                Some(rows.remove(at))
            })
            .map_or(1, |row| {
                let grants = self.grants.clone();
                tokio::spawn(async move {
                    let _ = grants.pending_remove(&row.turn_id, &row.request_id).await;
                });
                row.version as i32
            });
        let (tx, rx) = oneshot::channel();
        let action = request.action.clone();

        let timeout = Duration::from_secs(self.timeout_secs.max(1) as u64);
        let notification = ApprovalRequestedNotification {
            turn_id: self.turn_id.clone(),
            request_id: request_id.clone(),
            tool: request.tool,
            args_preview: request.args_preview,
            reason: request.reason,
            expires_in_ms: narrow(timeout.as_millis().min(i64::MAX as u128) as i64),
            version,
        };
        let waiting = Waiting::new(tx, request.remember.clone(), notification.clone(), timeout);
        let started = waiting.started;
        if let Ok(mut table) = self.pending.lock() {
            table
                .entry(self.turn_id.clone())
                .or_default()
                .insert(request_id.clone(), waiting);
        }
        let tool = notification.tool.clone();
        let args_preview = notification.args_preview.clone();
        let reason = notification.reason.clone();
        if let Some(notifier) = &self.notify {
            notifier.notify(jeeves_tools::notify::NotifyEvent::ApprovalRequested {
                turn_id: self.turn_id.clone(),
                tool: tool.clone(),
                args_preview: args_preview.clone(),
                reason: reason.clone(),
            });
        }
        self.sink
            .notification(ServerNotification::ApprovalRequested(notification));

        let pending = self.pending.clone();
        let turn_id = self.turn_id.clone();
        let logbook = self.logbook.clone();
        let grants = self.grants.clone();
        let runs = self.runs.clone();
        let events = self.events.clone();
        let push = self.push.clone();
        let row = jeeves_state::grants::PendingRow {
            turn_id: self.turn_id.clone(),
            request_id: request_id.clone(),
            session_id: self.session_id.clone(),
            tool: tool.clone(),
            args_preview: args_preview.clone(),
            reason: reason.clone(),
            version: i64::from(version),
            expires_at: chrono::Utc::now().timestamp() + timeout.as_secs() as i64,
        };

        Box::pin(async move {
            if let Err(error) = grants.pending_put(&row).await {
                eprintln!("(jeeves) the pending approval was not stored durably: {error}");
                if action.is_some() {
                    if let Some(waiting) = pending.lock().expect("approvals lock").get_mut(&turn_id)
                    {
                        waiting.remove(&request_id);
                    }
                    return false;
                }
            }
            let run = match runs.for_turn(&turn_id).await {
                Ok(run) => run,
                Err(error) => {
                    eprintln!("(jeeves) approval task lookup failed: {error}");
                    None
                }
            };
            if let Some(run) = &run {
                if let Some(server) = push {
                    crate::approval_push::notify(server, &run.task_id, &row);
                }
                let detail = format!("{tool} · {request_id}");
                if let Err(error) = events
                    .append(
                        &run.task_id,
                        Some(&run.id),
                        "approval_requested",
                        Some(&detail),
                    )
                    .await
                {
                    eprintln!("(jeeves) approval request event could not be recorded: {error}");
                }
            }
            let (outcome, actor) =
                match tokio::time::timeout(timeout.saturating_sub(started.elapsed()), rx).await {
                    Ok(Ok((true, actor))) => ("approved", actor),
                    Ok(Ok((false, actor))) => ("denied", actor),
                    Ok(Err(_)) => ("voided", String::new()),
                    Err(_) => ("timeout", String::new()),
                };
            if let Err(e) = logbook
                .log(&jeeves_state::approvals::Decision {
                    turn_id: &turn_id,
                    request_id: &request_id,
                    tool: &tool,
                    args_preview: &args_preview,
                    reason: &reason,
                    outcome,
                    actor: &actor,
                })
                .await
            {
                eprintln!("(jeeves) the approval audit record was not written ({outcome}): {e}");
            }
            if let Some(run) = &run {
                let detail = format!("{tool} · {outcome} · {actor}");
                if let Err(error) = events
                    .append(
                        &run.task_id,
                        Some(&run.id),
                        "approval_decided",
                        Some(&detail),
                    )
                    .await
                {
                    eprintln!("(jeeves) approval decision event could not be recorded: {error}");
                }
            }
            let mut approved = outcome == "approved";
            if approved {
                if let Some(action) = action {
                    approved = persist_grant(&grants, action).await;
                }
            }
            let _ = grants.pending_remove(&turn_id, &request_id).await;
            if let Ok(mut table) = pending.lock() {
                if let Some(per_turn) = table.get_mut(&turn_id) {
                    per_turn.remove(&request_id);
                    if per_turn.is_empty() {
                        table.remove(&turn_id);
                    }
                }
            }
            if !approved {
                eprintln!("(jeeves) approval request {request_id} was not approved in time; treating it as a denial");
            }
            approved
        })
    }
}

/// Persist the human's scope. A `once` grant is recorded for audit and immediately revoked, so
/// it can never authorize a second send.
async fn persist_grant(grants: &jeeves_state::Grants, action: ActionGrant) -> bool {
    if action
        .expires_at
        .is_some_and(|at| at <= chrono::Utc::now().timestamp())
    {
        eprintln!("(jeeves) approved action scope has expired; refusing the effect");
        return false;
    }
    let id = format!(
        "grant-{}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_micros(),
        next_request_id()
    );
    let grant = jeeves_state::Grant {
        id: id.clone(),
        account: action.account,
        resource: action.resource,
        action: action.action,
        destination: action.destination,
        content_hash: action.content_hash,
        scope: action.scope.clone(),
        expires_at: action.expires_at,
        revoked_version: i64::from(action.scope == "once"),
        created_at: chrono::Utc::now().timestamp(),
    };
    if let Err(error) = grants.add(&grant).await {
        eprintln!("(jeeves) the approved action grant could not be stored: {error}");
        return false;
    }
    eprintln!(
        "(jeeves) action grant {id} recorded for {}:{} -> {} ({})",
        grant.resource, grant.action, grant.destination, grant.scope
    );
    true
}

/// Counter complements process/time identity so old clients cannot answer a new process's question.
fn next_request_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
