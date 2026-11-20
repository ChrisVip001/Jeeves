//! Explicit human answers from the owner chat. Models have no approval tool.
use super::{internal, invalid};
use crate::Server;
use jeeves_protocol::{ErrorObject, InteractionStartParams, RememberScope};

pub(super) async fn answer(
    server: &Server,
    p: &InteractionStartParams,
    may_dispatch: bool,
) -> Result<Option<String>, ErrorObject> {
    let mut words = p.message.split_whitespace();
    let approved = match words.next() {
        Some("approve" | "批准") => true,
        Some("deny" | "拒绝") => false,
        _ => return Ok(None),
    };
    let Some(id) = words.next().filter(|w| w.starts_with("ap-")) else {
        return Ok(None);
    };
    if !may_dispatch || p.source.is_some() {
        return Err(invalid("Use the owner main chat to answer this approval"));
    }
    let version = words
        .next()
        .and_then(|v| v.strip_prefix('v'))
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|_| words.next().is_none())
        .ok_or_else(|| invalid("Include the exact request ID and version shown in the decision"))?;
    let target:Option<(String,String)>=sqlx::query_as("SELECT p.turn_id,r.task_id FROM pending_approvals p JOIN task_runs r ON r.turn_id=p.turn_id JOIN agent_tasks t ON t.id=r.task_id WHERE p.request_id=? AND p.version=? AND p.expires_at>unixepoch() AND t.session_id=? AND (? IS NULL OR t.id=?)").bind(id).bind(version).bind(&p.session_id).bind(&p.task_id).bind(&p.task_id).fetch_optional(&server.pool).await.map_err(internal)?;
    let Some((turn, task)) = target else {
        return Ok(Some(
            "此审批已过期或版本已变更。 Approval unavailable or superseded.".into(),
        ));
    };
    let accepted = server
        .answer_approval_checked(
            &turn,
            id,
            approved,
            RememberScope::Once,
            Some(version),
            crate::server::controls::OWNER,
        )
        .map_err(invalid)?;
    Ok(Some(if accepted {
        format!(
            "{} · {task} · {id} v{version}",
            if approved {
                "已批准本次操作 / Approved once"
            } else {
                "已拒绝 / Denied"
            }
        )
    } else {
        "此审批已结束。 Approval already closed.".into()
    }))
}
