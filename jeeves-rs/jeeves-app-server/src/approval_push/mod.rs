//! A push wakes the cockpit. Only its authenticated operator can view or answer the live question.
mod delivery;
mod subscription;
mod wire;
pub(crate) use delivery::notify;
pub(crate) use subscription::{forget, public_key, subscribe};

use crate::{dispatch::internal, Server};
use base64::{engine::general_purpose::STANDARD, Engine};
use jeeves_protocol::*;
use ring::signature::{UnparsedPublicKey, ED25519};
use sqlx::SqliteConnection;

fn signed(p: &ApprovalNoticeParams, approved: Option<bool>, proof: &str) -> bool {
    if p.device_id.len() != 44 || proof.len() != 88 {
        return false;
    }
    let (Ok(key), Ok(signature)) = (STANDARD.decode(&p.device_id), STANDARD.decode(proof)) else {
        return false;
    };
    let payload = serde_json::json!([
        "jeeves.approval.notice.v1",
        p.device_id,
        p.grant_id,
        p.generation,
        p.task_id,
        p.turn_id,
        p.approval_request_id,
        p.approval_version,
        approved
    ]);
    UnparsedPublicKey::new(&ED25519, key)
        .verify(payload.to_string().as_bytes(), &signature)
        .is_ok()
}
async fn issued(
    conn: &mut SqliteConnection,
    p: &ApprovalNoticeParams,
) -> Result<bool, ErrorObject> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM approval_push_attempts WHERE grant_id=? AND task_id=? AND turn_id=? AND request_id=? AND version=?)")
      .bind(&p.grant_id).bind(&p.task_id).bind(&p.turn_id).bind(&p.approval_request_id).bind(p.approval_version).fetch_one(conn).await.map_err(internal)
}

async fn granted(
    conn: &mut SqliteConnection,
    p: &ApprovalNoticeParams,
) -> Result<bool, ErrorObject> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM device_grants WHERE device_id=? AND grant_id=? AND generation=? AND revoked=0 AND EXISTS(SELECT 1 FROM json_each(capabilities) WHERE value='approvalReview'))")
        .bind(&p.device_id).bind(&p.grant_id).bind(p.generation).fetch_one(conn).await.map_err(internal)
}

fn live(
    server: &Server,
    p: &ApprovalNoticeParams,
) -> Result<Option<ApprovalRequestedNotification>, ErrorObject> {
    let pending = server
        .approvals
        .lock()
        .map_err(|_| internal("Pending approvals unavailable"))?;
    let Some(w) = pending
        .get(&p.turn_id)
        .and_then(|rows| rows.get(&p.approval_request_id))
    else {
        return Ok(None);
    };
    Ok(w.snapshot().filter(|ask| ask.version == p.approval_version))
}

pub(crate) async fn notice(
    server: &Server,
    check: ApprovalNoticeCheckParams,
) -> Result<ApprovalNoticeView, ErrorObject> {
    let unavailable = |status| ApprovalNoticeView {
        status,
        approval: None,
    };
    let p = check.notice;
    if !signed(&p, None, &check.signature) {
        return Ok(unavailable(ApprovalNoticeStatus::Revoked));
    }
    let mut conn = server.pool.acquire().await.map_err(internal)?;
    if !granted(&mut conn, &p).await? {
        return Ok(unavailable(ApprovalNoticeStatus::Revoked));
    }
    if !issued(&mut conn, &p).await? {
        return Ok(unavailable(ApprovalNoticeStatus::Expired));
    }
    drop(conn);
    let Some(task) = server
        .tasks
        .by_turn(&p.turn_id)
        .await
        .map_err(internal)?
        .filter(|task| task.id == p.task_id)
    else {
        return Ok(unavailable(ApprovalNoticeStatus::Expired));
    };
    let Some((_, session_id, _)) = server
        .active_turns(None)
        .into_iter()
        .find(|(turn, _, _)| turn == &p.turn_id)
    else {
        return Ok(unavailable(ApprovalNoticeStatus::Expired));
    };
    let owner_session_id = server.permissions.owner(&session_id).map_err(internal)?;
    let task = crate::task::view(server, &task).await?;
    let mut conn = server.pool.acquire().await.map_err(internal)?;
    if !granted(&mut conn, &p).await? {
        return Ok(unavailable(ApprovalNoticeStatus::Revoked));
    }
    let Some(request) = live(server, &p)? else {
        return Ok(unavailable(ApprovalNoticeStatus::Expired));
    };
    Ok(ApprovalNoticeView {
        status: ApprovalNoticeStatus::Pending,
        approval: Some(PendingApprovalView {
            task: Some(task),
            session_id,
            owner_session_id,
            request,
            restored: false,
        }),
    })
}

pub(crate) async fn decision(
    server: &Server,
    p: ApprovalNoticeDecisionParams,
) -> Result<ApprovalRespondResponse, ErrorObject> {
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let n = &p.notice;
    let matches: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM agent_tasks WHERE id=? AND current_turn_id=?)",
    )
    .bind(&n.task_id)
    .bind(&n.turn_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(internal)?;
    let accepted = signed(n, Some(p.approved), &p.signature)
        && matches
        && issued(&mut tx, n).await?
        && granted(&mut tx, n).await?
        && live(server, n)?.is_some()
        && server.answer_approval_as(
            &n.turn_id,
            &n.approval_request_id,
            p.approved,
            RememberScope::Once,
            &format!("pwa:{}", n.grant_id),
        );
    tx.commit().await.map_err(internal)?;
    Ok(ApprovalRespondResponse {
        accepted,
        answered_by: None,
    })
}

#[cfg(test)]
mod tests;
