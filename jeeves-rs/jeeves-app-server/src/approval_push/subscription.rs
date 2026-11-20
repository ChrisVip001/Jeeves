use crate::{dispatch::internal, Server};
use anyhow::{anyhow, ensure, Result};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64},
    Engine,
};
use jeeves_protocol::*;
use ring::{
    rand::SystemRandom,
    signature::{self, EcdsaKeyPair, KeyPair},
};
use serde::{Deserialize, Serialize};

const SIGNER: &str = "webpush:vapid";
pub(super) fn slot(grant: &str, sequence: i32) -> String {
    format!("webpush:sub:{grant}:{sequence}")
}

pub(super) fn signer(server: &Server, create: bool) -> Result<EcdsaKeyPair> {
    let home = &server.boot_settings.home;
    let mut store = server
        .secrets
        .lock()
        .map_err(|_| anyhow!("Credential store unavailable"))?;
    let rng = SystemRandom::new();
    let algorithm = &signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let der = match store.get(home, SIGNER) {
        Some(value) => B64
            .decode(value)
            .map_err(|_| anyhow!("Invalid stored VAPID key"))?,
        None if create => {
            let der = EcdsaKeyPair::generate_pkcs8(algorithm, &rng)
                .map_err(|_| anyhow!("Cannot create VAPID key"))?;
            store
                .put(home, SIGNER, &B64.encode(der.as_ref()))
                .map_err(|_| anyhow!("Cannot save VAPID key"))?;
            der.as_ref().to_vec()
        }
        None => anyhow::bail!("VAPID key unavailable"),
    };
    EcdsaKeyPair::from_pkcs8(algorithm, &der, &rng).map_err(|_| anyhow!("Invalid VAPID key"))
}

pub(crate) fn public_key(server: &Server) -> Result<ApprovalPushKeyResponse, ErrorObject> {
    let Some(contact) = server.boot_settings.web_push_contact.as_deref() else {
        return Ok(ApprovalPushKeyResponse { public_key: None });
    };
    let url = reqwest::Url::parse(contact)
        .map_err(|_| internal("Expected a mailto or HTTPS Web Push contact"))?;
    if !(url.scheme() == "mailto" && url.path().contains('@')
        || url.scheme() == "https" && url.host().is_some())
    {
        return Err(internal("Expected a mailto or HTTPS Web Push contact"));
    }
    let key = signer(server, true).map_err(internal)?;
    Ok(ApprovalPushKeyResponse {
        public_key: Some(B64.encode(key.public_key().as_ref())),
    })
}

#[derive(Serialize, Deserialize)]
pub(super) struct Subscription {
    pub(super) endpoint: String,
    pub(super) p256dh: String,
    pub(super) auth: String,
}
impl Subscription {
    pub(super) fn endpoint(&self) -> Result<reqwest::Url> {
        let url =
            reqwest::Url::parse(&self.endpoint).map_err(|_| anyhow!("Invalid push endpoint"))?;
        ensure!(
            url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| host.ends_with(".push.apple.com"))
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && url.path() != "/",
            "Expected an Apple Web Push subscription"
        );
        Ok(url)
    }
    pub(super) fn keys(&self) -> Result<(Vec<u8>, Vec<u8>)> {
        let decode = |text: &str| {
            B64.decode(text)
                .map_err(|_| anyhow!("Invalid push subscription keys"))
        };
        let key = decode(&self.p256dh)?;
        let auth = decode(&self.auth)?;
        ensure!(
            key.len() == 65 && key[0] == 4 && auth.len() == 16,
            "Invalid push subscription keys"
        );
        Ok((key, auth))
    }
}

pub(crate) async fn subscribe(
    server: &Server,
    p: ApprovalPushSubscribeParams,
) -> Result<DeviceReportResponse, ErrorObject> {
    let denied = || ErrorObject {
        code: codes::INVALID_PARAMS,
        message: "Push subscription denied".into(),
        data: None,
    };
    if server.boot_settings.web_push_contact.is_none()
        || p.endpoint.len() > 4096
        || p.p256dh.len() != 87
        || p.auth.len() != 22
        || p.signature.len() != 88
        || p.device_id.len() != 44
    {
        return Err(denied());
    }
    let payload = serde_json::to_vec(&(
        "jeeves.approval.push.v1",
        &p.device_id,
        &p.grant_id,
        p.generation,
        p.sequence,
        &p.endpoint,
        &p.p256dh,
        &p.auth,
    ))
    .map_err(internal)?;
    signature::UnparsedPublicKey::new(
        &signature::ED25519,
        STANDARD.decode(&p.device_id).map_err(|_| denied())?,
    )
    .verify(
        &payload,
        &STANDARD.decode(&p.signature).map_err(|_| denied())?,
    )
    .map_err(|_| denied())?;
    let sub = Subscription {
        endpoint: p.endpoint,
        p256dh: p.p256dh,
        auth: p.auth,
    };
    sub.endpoint().map_err(internal)?;
    let (key, auth) = sub.keys().map_err(internal)?;
    super::wire::encrypt(&key, &auth, Vec::new()).map_err(internal)?;
    let mut tx = server
        .pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(internal)?;
    let old:Option<Option<i32>>=sqlx::query_scalar("SELECT push_sequence FROM device_grants WHERE device_id=? AND grant_id=? AND generation=? AND revoked=0 AND last_sequence<? AND EXISTS (SELECT 1 FROM json_each(capabilities) WHERE value='approvalReview')")
        .bind(&p.device_id).bind(&p.grant_id).bind(p.generation).bind(p.sequence).fetch_optional(&mut *tx).await.map_err(internal)?;
    let old = old.ok_or_else(denied)?;
    let secret = serde_json::to_string(&sub).map_err(internal)?;
    server
        .secrets
        .lock()
        .map_err(|_| internal("Credential store unavailable"))?
        .put(
            &server.boot_settings.home,
            &slot(&p.grant_id, p.sequence),
            &secret,
        )
        .map_err(|_| internal("Could not save push subscription"))?;
    sqlx::query("UPDATE device_grants SET last_sequence=?,push_sequence=? WHERE device_id=?")
        .bind(p.sequence)
        .bind(p.sequence)
        .bind(&p.device_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    tx.commit().await.map_err(internal)?;
    if let Some(seq) = old {
        forget(server, &p.grant_id, seq);
    }
    Ok(DeviceReportResponse {
        sequence: p.sequence,
    })
}
pub(crate) fn forget(server: &Server, grant: &str, seq: i32) {
    if !server.secrets.lock().is_ok_and(|mut store| {
        store
            .remove(&server.boot_settings.home, &slot(grant, seq))
            .is_ok()
    }) {
        eprintln!("(jeeves) could not remove inactive push subscription from the credential store");
    }
}
