use super::{
    live,
    subscription::{self, Subscription},
    wire,
};
use crate::Server;
use anyhow::{anyhow, Context, Result};
use jeeves_protocol::ApprovalNoticeParams;
use jeeves_state::{
    devices::{self, DeviceGrant},
    grants::PendingRow,
};
use std::{future::Future, time::Duration};

pub(crate) fn notify(server: Server, task: &str, row: &PendingRow) {
    let task = task.to_owned();
    let row = row.clone();
    tokio::spawn(async move {
        let result = async {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()?;
            deliver(&server, &task, &row, |request| async {
                client
                    .execute(request)
                    .await
                    .map(|response| response.status())
                    .map_err(|e| anyhow!(e.without_url()))
            })
            .await
        }
        .await;
        if let Err(err) = result {
            eprintln!("(jeeves) approval notification unavailable: {err}");
        }
    });
}

pub(super) async fn deliver<F, Fut>(
    server: &Server,
    task: &str,
    row: &PendingRow,
    send: F,
) -> Result<()>
where
    F: Fn(reqwest::Request) -> Fut,
    Fut: Future<Output = Result<reqwest::StatusCode>>,
{
    let Some(subject) = server.boot_settings.web_push_contact.as_deref() else {
        return Ok(());
    };
    let devices = devices::list(&server.pool).await?;
    for device in devices {
        let Some(seq) = device.push_sequence else {
            continue;
        };
        if let Err(error) = deliver_one(server, task, row, subject, &device, seq, &send).await {
            eprintln!("(jeeves) approval notification delivery failed: {error}");
        }
    }
    Ok(())
}

async fn deliver_one<F, Fut>(
    server: &Server,
    task: &str,
    row: &PendingRow,
    subject: &str,
    device: &DeviceGrant,
    seq: i32,
    send: &F,
) -> Result<()>
where
    F: Fn(reqwest::Request) -> Fut,
    Fut: Future<Output = Result<reqwest::StatusCode>>,
{
    let locator = ApprovalNoticeParams {
        device_id: device.device_id.clone(),
        grant_id: device.grant_id.clone(),
        generation: device.generation,
        task_id: task.into(),
        turn_id: row.turn_id.clone(),
        approval_request_id: row.request_id.clone(),
        approval_version: i32::try_from(row.version)?,
    };
    let secret = server
        .secrets
        .lock()
        .map_err(|_| anyhow!("Credential store unavailable"))?
        .get(
            &server.boot_settings.home,
            &subscription::slot(&device.grant_id, seq),
        )
        .context("Missing push subscription")?;
    let sub: Subscription =
        serde_json::from_str(&secret).map_err(|_| anyhow!("Invalid stored push subscription"))?;
    let endpoint = sub.endpoint()?;
    let (receiver, auth) = sub.keys()?;
    let encrypted = wire::encrypt(&receiver, &auth, serde_json::to_vec(&locator)?)?;
    let auth_header =
        wire::authorization(&subscription::signer(server, false)?, &endpoint, subject)?;
    let mut request = reqwest::Request::new(reqwest::Method::POST, endpoint);
    let headers = request.headers_mut();
    headers.insert("content-encoding", "aes128gcm".parse()?);
    headers.insert("content-type", "application/octet-stream".parse()?);
    headers.insert("authorization", auth_header.parse()?);
    *request.body_mut() = Some(encrypted.into());
    let now = chrono::Utc::now().timestamp();
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("DELETE FROM approval_push_attempts WHERE expires_at<=?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    let Some(ask) = live(server, &locator).map_err(|e| anyhow!(e.message))? else {
        return Ok(());
    };
    if row.expires_at <= now {
        return Ok(());
    }
    request
        .headers_mut()
        .insert("ttl", ((ask.expires_in_ms / 1000) as u32).into());
    let claim=sqlx::query("INSERT INTO approval_push_attempts(grant_id,task_id,turn_id,request_id,version,expires_at)
      SELECT grant_id,?,?,?,?,? FROM device_grants WHERE device_id=? AND grant_id=? AND generation=? AND revoked=0 AND push_sequence=?
      AND EXISTS(SELECT 1 FROM json_each(capabilities) WHERE value='approvalReview') ON CONFLICT DO NOTHING")
        .bind(task).bind(&row.turn_id).bind(&row.request_id).bind(row.version).bind(row.expires_at)
        .bind(&device.device_id).bind(&device.grant_id).bind(device.generation).bind(seq).execute(&mut *tx).await?;
    tx.commit().await?;
    if claim.rows_affected() == 0 {
        return Ok(());
    }
    let status = send(request).await?;
    if matches!(
        status,
        reqwest::StatusCode::NOT_FOUND | reqwest::StatusCode::GONE
    ) {
        let updated = sqlx::query(
            "UPDATE device_grants SET push_sequence=NULL WHERE grant_id=? AND push_sequence=?",
        )
        .bind(&device.grant_id)
        .bind(seq)
        .execute(&server.pool)
        .await?;
        if updated.rows_affected() != 0 {
            subscription::forget(server, &device.grant_id, seq);
        }
    }
    anyhow::ensure!(status.is_success(), "push service returned {status}");
    Ok(())
}
