//! Operator RPCs manage public keys. A device can only report its own granted capability.
use crate::rpc_error::{internal, invalid};
use base64::{Engine, engine::general_purpose::STANDARD};
use jeeves_protocol::{
    DeviceGrantView, DeviceListResponse, DevicePairParams, DeviceReportParams,
    DeviceReportResponse, DeviceRevokeParams, ErrorObject,
};
use jeeves_state::devices::{self, DeviceGrant};
use ring::signature::{ED25519, UnparsedPublicKey};
use sqlx::SqlitePool;

fn public_key(id: &str) -> Result<[u8; 32], ErrorObject> {
    let mut key = [0; 32];
    if id.len() != 44 || STANDARD.decode_slice(id, &mut key).ok() != Some(key.len()) {
        return Err(invalid("Expected a base64-encoded Ed25519 public key"));
    }
    Ok(key)
}

fn view(row: DeviceGrant) -> Result<DeviceGrantView, ErrorObject> {
    Ok(DeviceGrantView {
        device_id: row.device_id,
        grant_id: row.grant_id,
        generation: row.generation,
        capabilities: serde_json::from_str(&row.capabilities).map_err(internal)?,
        revoked: row.revoked,
        last_sequence: row.last_sequence,
    })
}

pub(super) async fn list(pool: &SqlitePool) -> Result<DeviceListResponse, ErrorObject> {
    let devices = devices::list(pool)
        .await
        .map_err(internal)?
        .into_iter()
        .map(view)
        .collect::<Result<_, _>>()?;
    Ok(DeviceListResponse { devices })
}

pub(super) async fn pair(
    server: &crate::Server,
    mut p: DevicePairParams,
) -> Result<DeviceGrantView, ErrorObject> {
    public_key(&p.device_id)?;
    if p.capabilities.is_empty() {
        return Err(invalid("Select at least one device capability"));
    }
    p.capabilities.sort_unstable();
    p.capabilities.dedup();
    let caps = serde_json::to_string(&p.capabilities).map_err(internal)?;
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let previous = old_subscription(&mut tx, &p.device_id).await?;
    let row = devices::pair(&mut tx, &p.device_id, p.expected_generation, &caps)
        .await
        .map_err(internal)?
        .ok_or_else(|| invalid("Device grant changed; reload the device list"))?;
    tx.commit().await.map_err(internal)?;
    if let Some((grant, seq)) = previous {
        crate::approval_push::forget(server, &grant, seq);
    }
    view(row)
}

pub(super) async fn revoke(
    server: &crate::Server,
    p: DeviceRevokeParams,
) -> Result<DeviceGrantView, ErrorObject> {
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let previous = old_subscription(&mut tx, &p.device_id).await?;
    let row = devices::revoke(&mut tx, &p.device_id, p.expected_generation)
        .await
        .map_err(internal)?
        .ok_or_else(|| invalid("No active device grant at this generation"))?;
    tx.commit().await.map_err(internal)?;
    if let Some((grant, seq)) = previous {
        crate::approval_push::forget(server, &grant, seq);
    }
    view(row)
}

async fn old_subscription(
    tx: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<Option<(String, i32)>, ErrorObject> {
    sqlx::query_as("SELECT grant_id,push_sequence FROM device_grants WHERE device_id=? AND push_sequence IS NOT NULL").bind(id).fetch_optional(tx).await.map_err(internal)
}

pub(super) async fn report(
    pool: &SqlitePool,
    p: DeviceReportParams,
) -> Result<DeviceReportResponse, ErrorObject> {
    let denied = || invalid("Device report denied");
    let key = public_key(&p.device_id)?;
    let mut signature = [0; 64];
    if p.signature.len() != 88
        || STANDARD.decode_slice(&p.signature, &mut signature).ok() != Some(signature.len())
    {
        return Err(denied());
    }
    let payload = serde_json::to_vec(&(
        "jeeves.device.report.v1",
        &p.device_id,
        &p.grant_id,
        p.generation,
        p.capability,
        p.sequence,
    ))
    .map_err(internal)?;
    UnparsedPublicKey::new(&ED25519, key)
        .verify(&payload, &signature)
        .map_err(|_| denied())?;
    let cap = serde_json::to_string(&p.capability).map_err(internal)?;
    if !devices::report(
        pool,
        &p.device_id,
        &p.grant_id,
        p.generation,
        &cap,
        p.sequence,
    )
    .await
    .map_err(internal)?
    {
        return Err(denied());
    }
    Ok(DeviceReportResponse {
        sequence: p.sequence,
    })
}
