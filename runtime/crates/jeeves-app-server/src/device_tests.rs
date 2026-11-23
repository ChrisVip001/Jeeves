use crate::input_wake_tests::{fixture, rpc};
use serde_json::json;

#[tokio::test]
async fn device_pairing_requires_the_current_generation() {
    let (_home, server, _) = fixture().await;
    let params = json!({
        "deviceId":"11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=",
        "expectedGeneration":0,
        "capabilities":["browserControl"]
    });
    let paired = rpc(&server, "device/pair", params.clone()).await.unwrap();
    assert_eq!(paired["generation"], 1);
    assert!(rpc(&server, "device/pair", params).await.is_err());
}

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::Value;

fn signed(key: &Ed25519KeyPair, grant: &Value, cap: &str, seq: i32) -> Value {
    let (id, epoch, generation) = (&grant["deviceId"], &grant["grantId"], &grant["generation"]);
    let payload = json!(["jeeves.device.report.v1", id, epoch, generation, cap, seq]);
    json!({"deviceId":id, "grantId":epoch, "generation":generation, "capability":cap,
        "sequence":seq, "signature":STANDARD.encode(key.sign(payload.to_string().as_bytes()).as_ref())})
}

#[tokio::test]
async fn device_pairing_binds_signer_capability_epoch_and_sequence_across_restart() {
    let home = tempfile::tempdir().unwrap();
    let config = jeeves_config::Settings {
        home: home.path().into(),
        provider: "test-unconfigured".into(),
        ..Default::default()
    };
    let server = crate::open(&config).await.unwrap();
    let phone = Ed25519KeyPair::from_seed_unchecked(&[1; 32]).unwrap();
    let mac = Ed25519KeyPair::from_seed_unchecked(&[2; 32]).unwrap();
    let phone_id = STANDARD.encode(phone.public_key().as_ref());
    let mac_id = STANDARD.encode(mac.public_key().as_ref());
    let mut pair =
        json!({"deviceId": phone_id, "expectedGeneration":0, "capabilities":["approvalReview"]});
    let grant = rpc(&server, "device/pair", pair.clone()).await.unwrap();
    let unpaired = json!({"deviceId":mac_id,"grantId":grant["grantId"],"generation":1});
    for invalid in [
        signed(&mac, &unpaired, "desktopControl", 1),
        signed(&mac, &grant, "approvalReview", 1),
        signed(&phone, &grant, "desktopControl", 1),
    ] {
        assert!(rpc(&server, "device/report", invalid).await.is_err());
    }
    let desktop = rpc(
        &server,
        "device/pair",
        json!({"deviceId":mac_id,"expectedGeneration":0,"capabilities":["desktopControl"]}),
    )
    .await
    .unwrap();
    let valid = signed(&phone, &grant, "approvalReview", 1);
    let (first, duplicate) = tokio::join!(
        rpc(&server, "device/report", valid.clone()),
        rpc(&server, "device/report", valid.clone())
    );
    assert_ne!(first.is_ok(), duplicate.is_ok());
    let late = signed(&phone, &grant, "approvalReview", 2);
    let revoked = rpc(
        &server,
        "device/revoke",
        json!({"deviceId":phone_id,"expectedGeneration":1}),
    )
    .await
    .unwrap();
    assert_eq!(revoked["generation"], 2);
    drop(server);
    let server = crate::open(&config).await.unwrap();
    let devices = rpc(&server, "device/list", json!({})).await.unwrap();
    let saved = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["deviceId"] == phone_id)
        .unwrap();
    assert_eq!(saved["revoked"], true);
    assert_eq!(saved["lastSequence"], 1);
    assert!(rpc(&server, "device/report", late.clone()).await.is_err());
    assert!(rpc(&server, "device/pair", pair.clone()).await.is_err());
    pair["expectedGeneration"] = json!(2);
    let renewed = rpc(&server, "device/pair", pair).await.unwrap();
    assert_eq!(renewed["generation"], 3);
    assert_ne!(renewed["grantId"], grant["grantId"]);
    assert!(rpc(&server, "device/report", late).await.is_err());
    let wrong_epoch = json!({"grantId":grant["grantId"], "generation":3, "deviceId":phone_id});
    assert!(
        rpc(
            &server,
            "device/report",
            signed(&phone, &wrong_epoch, "approvalReview", 1)
        )
        .await
        .is_err()
    );
    for (key, row, cap) in [
        (&phone, &renewed, "approvalReview"),
        (&mac, &desktop, "desktopControl"),
    ] {
        assert_eq!(
            rpc(&server, "device/report", signed(key, row, cap, 1))
                .await
                .unwrap()["sequence"],
            1
        );
    }
}
