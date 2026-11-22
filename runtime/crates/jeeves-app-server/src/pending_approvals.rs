//! A live snapshot, never reconstructed from approval audit history.
use crate::{narrow, Server};
use jeeves_protocol::{codes, ApprovalPendingResponse, ErrorObject, PendingApprovalView};

pub(crate) async fn list(
    server: &Server,
    session: &str,
) -> Result<ApprovalPendingResponse, ErrorObject> {
    if server
        .chat
        .session(session)
        .await
        .map_err(invalid)?
        .is_none()
    {
        return Err(invalid("Conversation no longer exists"));
    }
    let owner = server.permissions.owner(session).map_err(invalid)?;
    let mut owned = Vec::new();
    for (turn, execution, _) in server.active_turns(None) {
        if server.permissions.owner(&execution).map_err(invalid)? == owner {
            owned.push((turn, execution));
        }
    }
    let mut requests = Vec::new();
    {
        let pending = server
            .approvals
            .lock()
            .map_err(|_| invalid("Pending approvals unavailable"))?;
        for (turn, execution) in owned {
            if let Some(waiting) = pending.get(&turn) {
                for entry in waiting.values() {
                    if let Some(request) = entry.snapshot() {
                        requests.push((execution.clone(), request));
                    }
                }
            }
        }
    }
    let mut approvals = Vec::new();
    for (execution, request) in requests {
        let task = match server
            .tasks
            .by_turn(&request.turn_id)
            .await
            .map_err(invalid)?
        {
            Some(task) => match crate::task::view(server, &task).await {
                Ok(view) => Some(view),
                Err(error) => {
                    eprintln!(
                        "(jeeves) optional task details for pending approval unavailable: {}",
                        error.message
                    );
                    None
                }
            },
            None => None,
        };
        approvals.push(PendingApprovalView {
            session_id: execution,
            owner_session_id: owner.clone(),
            restored: false,
            task,
            request,
        });
    }
    let leftovers: Vec<_> = server
        .restored_approvals
        .lock()
        .map_err(|_| invalid("Pending approvals unavailable"))?
        .clone();
    let now = chrono::Utc::now().timestamp();
    for row in leftovers {
        if row.expires_at <= now
            || server.permissions.owner(&row.session_id).map_err(invalid)? != owner
        {
            continue;
        }
        approvals.push(PendingApprovalView {
            session_id: row.session_id,
            owner_session_id: owner.clone(),
            restored: true,
            task: None,
            request: jeeves_protocol::ApprovalRequestedNotification {
                turn_id: row.turn_id,
                request_id: row.request_id,
                tool: row.tool,
                args_preview: row.args_preview,
                reason: row.reason,
                expires_in_ms: narrow((row.expires_at - now).max(0) * 1000),
                version: row.version as i32,
            },
        });
    }
    approvals.sort_by(|a, b| {
        a.request
            .turn_id
            .cmp(&b.request.turn_id)
            .then(a.request.request_id.cmp(&b.request.request_id))
    });
    Ok(ApprovalPendingResponse { approvals })
}
fn invalid(error: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: error.to_string(),
        data: None,
    }
}
