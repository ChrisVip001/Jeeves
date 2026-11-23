use super::*;

fn gateways() -> (tempfile::TempDir, Arc<Gateways>) {
    let dir = tempfile::tempdir().expect("temp dir");
    let gateways = Gateways::with_secrets(dir.path(), None, SecretStore::file_only());
    (dir, gateways)
}

fn config(allow: &[&str]) -> LarkConfig {
    LarkConfig {
        enabled: true,
        app_id: " cli_abc ".into(),
        allow: allow.iter().map(|s| s.to_string()).collect(),
        ..LarkConfig::default()
    }
}

#[test]
fn settings_round_trip_and_the_secret_never_lands_in_the_json() {
    let (dir, gateways) = gateways();
    let landed = gateways.save(config(&["ou_1"]), Some("s3cret")).unwrap();
    assert_eq!(landed, Some("file"));
    let status = gateways.status();
    assert_eq!(status.config.app_id, "cli_abc");
    assert!(status.has_secret);
    assert_eq!(status.state, "stopped");
    let raw = std::fs::read_to_string(dir.path().join("gateways.json")).unwrap();
    assert!(!raw.contains("s3cret"), "{raw}");
    assert_eq!(gateways.save(config(&["ou_1"]), None).unwrap(), None);
    assert!(gateways.status().has_secret);
}

#[test]
fn an_allowlist_entry_that_is_not_an_open_id_is_refused() {
    let (dir, gateways) = gateways();
    let error = gateways.save(config(&["cli_abc"]), None).unwrap_err();
    assert!(error.to_string().contains("ou_"), "{error}");
    assert!(
        !dir.path().join("gateways.json").exists(),
        "nothing half-saved"
    );
}

#[tokio::test]
async fn start_refuses_without_credentials_and_says_why() {
    let (_dir, gateways) = gateways();
    gateways.set_url("http://127.0.0.1:1".into());
    assert!(
        gateways
            .start()
            .await
            .unwrap_err()
            .to_string()
            .contains("App ID")
    );
    gateways.save(config(&[]), None).unwrap();
    assert!(
        gateways
            .start()
            .await
            .unwrap_err()
            .to_string()
            .contains("App Secret")
    );
}

#[tokio::test]
async fn only_the_waiting_sender_can_be_confirmed_as_owner() {
    let (_dir, gateways) = gateways();
    gateways.save(config(&[]), Some("s")).unwrap();
    assert!(gateways.claim("ou_x").await.is_err());
    gateways.live.lock().unwrap().candidate = Some("ou_a".into());
    assert!(
        gateways.claim("ou_b").await.is_err(),
        "a different id is not the candidate"
    );
    gateways.save(config(&["ou_owner"]), None).unwrap();
    assert!(
        gateways
            .claim("ou_a")
            .await
            .unwrap_err()
            .to_string()
            .contains("already")
    );
}
