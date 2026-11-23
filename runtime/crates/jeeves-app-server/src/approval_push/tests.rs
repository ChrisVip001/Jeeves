use super::*;
use crate::{
    input_wake_tests::{fixture, rpc},
    EventSink, Frame,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64},
    Engine,
};
use jeeves_state::grants::PendingRow;
use jeeves_tools::approval::{ApprovalOutcome, ApprovalRequest};
use ring::{
    agreement,
    rand::SystemRandom,
    signature::{self, Ed25519KeyPair, KeyPair},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn paired_phone_receives_an_encrypted_task_notice_and_reviews_once() {
    let (_home, mut server, _model) = fixture().await;
    server
        .secrets
        .lock()
        .unwrap()
        .set_backend(jeeves_config::models::SecretBackend::Memory);
    assert!(rpc(&server, "approval/pushKey", json!({})).await.unwrap()["publicKey"].is_null());
    Arc::make_mut(&mut server.boot_settings).web_push_contact =
        Some("mailto:fixture@example.invalid".into());
    let vapid = rpc(&server, "approval/pushKey", json!({})).await.unwrap()["publicKey"]
        .as_str()
        .unwrap()
        .to_owned();
    let phone = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    let id = STANDARD.encode(phone.public_key().as_ref());
    let grant = rpc(
        &server,
        "device/pair",
        json!({"deviceId":id,"expectedGeneration":0,"capabilities":["approvalReview"]}),
    )
    .await
    .unwrap();
    let receiver =
        agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &SystemRandom::new())
            .unwrap();
    let public = receiver.compute_public_key().unwrap();
    let p256 = B64.encode(public.as_ref());
    let auth = B64.encode([9; 16]);
    let epoch = grant["grantId"].as_str().unwrap();
    let subscribe = |endpoint: &str| {
        let msg = json!([
            "jeeves.approval.push.v1",
            id,
            epoch,
            1,
            1,
            endpoint,
            p256,
            auth
        ]);
        json!({"deviceId":id,"grantId":epoch,"generation":1,"sequence":1,"endpoint":endpoint,"p256dh":p256,"auth":auth,"signature":STANDARD.encode(phone.sign(msg.to_string().as_bytes()).as_ref())})
    };
    assert!(rpc(
        &server,
        "approval/pushSubscribe",
        subscribe("https://fixture.invalid/sub")
    )
    .await
    .is_err());
    let mut modified = subscribe("https://web.push.apple.com/fictional");
    modified["endpoint"] = json!("https://web.push.apple.com/modified");
    assert!(rpc(&server, "approval/pushSubscribe", modified)
        .await
        .is_err());
    let sub = subscribe("https://web.push.apple.com/fictional");
    assert_eq!(
        rpc(&server, "approval/pushSubscribe", sub.clone())
            .await
            .unwrap()["sequence"],
        1
    );
    assert!(rpc(&server, "approval/pushSubscribe", sub).await.is_err());
    server
        .tasks
        .set_running("input-task", "turn")
        .await
        .unwrap();
    server
        .chat
        .create_session("chat", None, None)
        .await
        .unwrap();
    server.begin_turn("turn", "chat").unwrap();
    let (sink, mut frames) = EventSink::channel();
    let broker = server.approval_bridge_forced("turn", &sink).unwrap();
    let waiting = broker.request(ApprovalRequest {
        tool: "fixture_write".into(),
        args_preview: "private scope".into(),
        reason: "fixture".into(),
        timeout_secs: 60,
        remember: None,
        action: None,
    });
    let Some(Frame::Notification(ServerNotification::ApprovalRequested(ask))) = frames.recv().await
    else {
        panic!("approval")
    };
    let row = PendingRow {
        request_id: ask.request_id.clone(),
        turn_id: ask.turn_id.clone(),
        version: ask.version.into(),
        session_id: "chat".into(),
        args_preview: ask.args_preview,
        reason: ask.reason,
        tool: ask.tool,
        expires_at: chrono::Utc::now().timestamp() + 60,
    };
    let sent = Mutex::new(Vec::new());
    for _ in 0..2 {
        delivery::deliver(&server, "input-task", &row, |request| {
            sent.lock().unwrap().push(request);
            std::future::ready(Err(anyhow::anyhow!("offline test transport")))
        })
        .await
        .unwrap();
    }
    let mut sent = sent.into_inner().unwrap();
    assert_eq!(
        sent.len(),
        1,
        "an unknown or offline delivery must not be repeated"
    );
    let message = sent.pop().unwrap();
    assert_eq!(message.headers()["content-encoding"], "aes128gcm");
    let authorization = message.headers()["authorization"].to_str().unwrap();
    let (jwt, pk) = authorization
        .strip_prefix("vapid t=")
        .unwrap()
        .split_once(", k=")
        .unwrap();
    assert_eq!(pk, vapid);
    let (claims, sig) = jwt.rsplit_once('.').unwrap();
    signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, B64.decode(pk).unwrap())
        .verify(claims.as_bytes(), &B64.decode(sig).unwrap())
        .unwrap();
    let claims: Value =
        serde_json::from_slice(&B64.decode(claims.split_once('.').unwrap().1).unwrap()).unwrap();
    assert_eq!(claims["aud"], "https://web.push.apple.com");
    let body = message.body().unwrap().as_bytes().unwrap();
    assert_eq!(&body[16..21], &[0, 0, 16, 0, 65]);
    let sender = &body[21..86];
    let locator: Value = agreement::agree_ephemeral(
        receiver,
        &agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, sender),
        |shared| {
            let (key, nonce) =
                super::wire::keys(shared, public.as_ref(), sender, &[9; 16], &body[..16]).unwrap();
            let mut data = body[86..].to_vec();
            let opened = ring::aead::LessSafeKey::new(
                ring::aead::UnboundKey::new(&ring::aead::AES_128_GCM, &key).unwrap(),
            )
            .open_in_place(
                ring::aead::Nonce::assume_unique_for_key(nonce),
                ring::aead::Aad::empty(),
                &mut data,
            )
            .unwrap();
            assert_eq!(opened.last(), Some(&2));
            serde_json::from_slice(&opened[..opened.len() - 1]).unwrap()
        },
    )
    .unwrap();
    assert_eq!(
        locator,
        json!({"deviceId":id,"grantId":epoch,"generation":1,"taskId":"input-task","turnId":"turn","approvalRequestId":row.request_id,"approvalVersion":1})
    );
    let sign = |mut n: Value, decision: Option<bool>| {
        let payload = json!([
            "jeeves.approval.notice.v1",
            n["deviceId"],
            n["grantId"],
            n["generation"],
            n["taskId"],
            n["turnId"],
            n["approvalRequestId"],
            n["approvalVersion"],
            decision
        ]);
        n["signature"] =
            json!(STANDARD.encode(phone.sign(payload.to_string().as_bytes()).as_ref()));
        if let Some(value) = decision {
            n["approved"] = json!(value)
        }
        n
    };
    let mut copied = locator.clone();
    copied["signature"] = json!(STANDARD.encode([1; 64]));
    assert_eq!(
        rpc(&server, "approval/notice", copied).await.unwrap()["status"],
        "revoked"
    );
    let view = rpc(&server, "approval/notice", sign(locator.clone(), None))
        .await
        .unwrap();
    assert_eq!(view["status"], "pending");
    assert_eq!(view["approval"]["request"]["argsPreview"], "private scope");
    let mut wrong = locator.clone();
    wrong["approvalVersion"] = json!(2);
    assert_eq!(
        rpc(&server, "approval/notice", sign(wrong.clone(), None))
            .await
            .unwrap()["status"],
        "expired"
    );
    let wrong = sign(wrong, Some(true));
    assert_eq!(
        rpc(&server, "approval/noticeDecision", wrong)
            .await
            .unwrap()["accepted"],
        false
    );
    let mut changed = sign(locator.clone(), Some(false));
    changed["approved"] = json!(true);
    assert_eq!(
        rpc(&server, "approval/noticeDecision", changed)
            .await
            .unwrap()["accepted"],
        false
    );
    let answer = sign(locator.clone(), Some(false));
    assert_eq!(
        rpc(&server, "approval/noticeDecision", answer.clone())
            .await
            .unwrap()["accepted"],
        true
    );
    assert_eq!(waiting.await, ApprovalOutcome::Denied);
    assert_eq!(
        rpc(&server, "approval/noticeDecision", answer)
            .await
            .unwrap()["accepted"],
        false
    );
    assert_eq!(
        rpc(&server, "approval/notice", sign(locator.clone(), None))
            .await
            .unwrap()["status"],
        "expired"
    );
    let waiting = broker.request(ApprovalRequest {
        tool: "fixture_write".into(),
        args_preview: "second file".into(),
        reason: "fixture".into(),
        timeout_secs: 60,
        remember: None,
        action: None,
    });
    let Some(Frame::Notification(ServerNotification::ApprovalRequested(second))) =
        frames.recv().await
    else {
        panic!("second approval")
    };
    let row = PendingRow {
        request_id: second.request_id.clone(),
        ..row
    };
    delivery::deliver(&server, "input-task", &row, |_| {
        std::future::ready(Ok(reqwest::StatusCode::CREATED))
    })
    .await
    .unwrap();
    let mut second_locator = locator.clone();
    second_locator["approvalRequestId"] = json!(second.request_id);
    assert_eq!(
        rpc(
            &server,
            "approval/notice",
            sign(second_locator.clone(), None)
        )
        .await
        .unwrap()["status"],
        "pending"
    );
    rpc(
        &server,
        "device/revoke",
        json!({"deviceId":id,"expectedGeneration":1}),
    )
    .await
    .unwrap();
    rpc(
        &server,
        "device/pair",
        json!({"deviceId":id,"expectedGeneration":2,"capabilities":["approvalReview"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        rpc(&server, "approval/notice", sign(locator, None))
            .await
            .unwrap()["status"],
        "revoked"
    );
    assert_eq!(
        rpc(
            &server,
            "approval/noticeDecision",
            sign(second_locator, Some(true))
        )
        .await
        .unwrap()["accepted"],
        false
    );
    assert!(server.answer_approval("turn", &second.request_id, false, RememberScope::Once));
    assert_eq!(waiting.await, ApprovalOutcome::Denied);
    assert!(server
        .secrets
        .lock()
        .unwrap()
        .get(&server.boot_settings.home, &subscription::slot(epoch, 1))
        .is_none());
}
