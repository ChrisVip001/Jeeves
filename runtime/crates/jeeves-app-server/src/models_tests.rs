//! Pure logic of `models.rs`: views, provider validation, the current label.
//!
//! The part that needs `Server` (save/switch/remove) is covered by jeeves-config's models unit tests
//! plus smoke coverage — this file only pins the three things that do not depend on process state.

use super::*;

fn profile(id: &str, provider: &str, key: Option<&str>) -> cfg::ModelProfile {
    cfg::ModelProfile {
        id: id.to_string(),
        provider: provider.to_string(),
        model: Some("m1".to_string()),
        small_model: None,
        base_url: None,
        credential_slot: Default::default(),
        key_source: key.map(|k| k.to_string()),
        vision: None,
        capabilities: None,
    }
}

#[test]
fn views_carry_the_key_pointer_but_never_the_secret() {
    let mut file = cfg::ModelsFile::default();
    cfg::upsert(&mut file, profile("ds", "deepseek", Some("keychain")));
    cfg::upsert(
        &mut file,
        profile("oa", "openai", Some("env:OPENAI_API_KEY")),
    );
    file.active = Some("ds".to_string());

    let out = views(&file);
    assert_eq!(out.len(), 2);
    assert!(out[0].active, "ds is the active profile");
    assert!(!out[1].active);
    assert_eq!(out[0].key_source.as_deref(), Some("keychain"));
    assert_eq!(out[1].key_source.as_deref(), Some("env:OPENAI_API_KEY"));
    let serialized = serde_json::to_string(&out).expect("serialize");
    assert!(
        !serialized.contains("sk-"),
        "the view must not carry keys: {serialized}"
    );
}

#[test]
fn provider_validation_accepts_builtin_and_custom_catalog_ids() {
    let catalog = jeeves_provider::catalog::Catalog::built_in();
    let known = catalog
        .ids()
        .first()
        .copied()
        .expect("at least one vendor in the catalog");
    assert!(provider_entry(&catalog, known).is_ok());

    let err = provider_entry(&catalog, "a vendor that does not exist").unwrap_err();
    assert!(err.contains("unknown provider"), "{err}");
    assert!(
        err.contains(known),
        "the error should list what is available: {err}"
    );

    let dir = tempfile::tempdir().expect("temporary directory");
    std::fs::write(
        dir.path().join("models-catalog.json"),
        r#"[{"id":"mybox","wire":"openai","model":"qwen3:8b","base_url":"http://127.0.0.1:9/v1"}]"#,
    )
    .expect("write the catalog file");
    let extended = jeeves_provider::catalog::Catalog::load(dir.path());
    assert!(provider_entry(&extended, "mybox").is_ok());
}

#[test]
fn the_current_label_names_provider_and_model() {
    let mut settings = jeeves_config::Settings {
        provider: "deepseek".to_string(),
        ..jeeves_config::Settings::default()
    };
    assert_eq!(current_label(&settings).as_deref(), Some("deepseek"));

    settings.model = Some("deepseek-flash".to_string());
    assert_eq!(
        current_label(&settings).as_deref(),
        Some("deepseek / deepseek-flash")
    );

    let empty = jeeves_config::Settings {
        provider: String::new(),
        ..jeeves_config::Settings::default()
    };
    assert_eq!(current_label(&empty), None);
}

async fn server() -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let server = Server::boot(
        pool,
        jeeves_config::Settings {
            home,
            ..jeeves_config::Settings::default()
        },
    )
    .await;
    server
        .secrets
        .lock()
        .expect("the secrets lock should not be poisoned")
        .set_backend(jeeves_config::models::SecretBackend::File);
    (server, dir)
}

fn save(id: &str, provider: &str, model: Option<&str>) -> ModelSaveParams {
    ModelSaveParams {
        id: id.to_string(),
        provider: Some(provider.to_string()),
        model: model.map(|m| m.to_string()),
        small_model: None,
        base_url: None,
        api_key: None,
        key_env: Some("JEEVES_TEST_KEY".to_string()),
        vision: None,
        capabilities: None,
        make_active: None,
    }
}

#[tokio::test]
async fn saving_switching_and_removing_end_to_end() {
    let (server, dir) = server().await;

    let first = server
        .model_save(save("ds", "deepseek", Some("deepseek-flash")))
        .expect("store the first profile");
    assert!(first.active, "once stored it should be the active one");
    assert_eq!(first.key_stored_in, "env:JEEVES_TEST_KEY");
    assert_eq!(first.current.as_deref(), Some("deepseek / deepseek-flash"));

    server
        .model_save(save("oa", "openai", Some("gpt-5.2")))
        .expect("store the second profile");
    let list = server.model_profiles();
    assert_eq!(
        list.active_id.as_deref(),
        Some("oa"),
        "the one saved later became current"
    );
    assert_eq!(list.profiles.len(), 2);
    assert!(list.profiles.iter().all(|p| p.key_source.is_some()));

    let switched = server
        .model_save(ModelSaveParams {
            provider: None,
            model: None,
            ..save("ds", "", None)
        })
        .expect("switch");
    assert!(switched.active);
    assert_eq!(
        server.model_profiles().active_id.as_deref(),
        Some("ds"),
        "switched back"
    );
    let back = server.model_profiles();
    let ds = back
        .profiles
        .iter()
        .find(|p| p.id == "ds")
        .expect("ds is still there");
    assert_eq!(ds.provider, "deepseek");
    assert_eq!(ds.model.as_deref(), Some("deepseek-flash"));

    let removed = server
        .model_remove(ModelRemoveParams {
            id: "ds".to_string(),
        })
        .unwrap();
    assert!(removed.removed);
    assert!(removed.active_id.is_none());

    let raw = std::fs::read_to_string(dir.path().join("models.json")).expect("models.json");
    assert!(!raw.contains("sk-"), "no key may appear in the file: {raw}");
    assert!(
        raw.contains("env:JEEVES_TEST_KEY"),
        "the credential pointer should be there: {raw}"
    );
}

#[tokio::test]
async fn an_unknown_provider_is_refused_with_the_list() {
    let (server, _dir) = server().await;
    let err = server
        .model_save(save("x", "a vendor that does not exist", None))
        .unwrap_err();
    assert!(err.contains("unknown provider"), "{err}");
    assert!(
        err.contains(jeeves_provider::PROVIDERS[0].id),
        "the error should list what is available: {err}"
    );
    assert!(
        server.model_profiles().profiles.is_empty(),
        "nothing got stored"
    );
}

#[tokio::test]
async fn a_new_profile_needs_a_provider() {
    let (server, _dir) = server().await;
    let err = server
        .model_save(ModelSaveParams {
            provider: None,
            ..save("bare", "", None)
        })
        .unwrap_err();
    assert!(err.contains("first configuration"), "{err}");
}

#[tokio::test]
async fn a_profile_can_declare_vision_over_the_vendor_default() {
    let (server, _dir) = server().await;
    server
        .model_save(ModelSaveParams {
            api_key: Some("sk-vision".to_string()),
            key_env: None,
            vision: Some(true),
            ..save("ds", "deepseek", Some("deepseek-v4.1-flash"))
        })
        .expect("store a profile that declares vision");

    assert!(
        server.resolved().expect("it should parse").vision,
        "the parsed result has to carry the declaration, or the door still blocks images"
    );
    assert_eq!(
        server.model_profiles().profiles[0].vision,
        Some(true),
        "the list has to show it honestly too"
    );

    server
        .model_save(ModelSaveParams {
            model: Some("deepseek-v4.1-flash".to_string()),
            key_env: None,
            ..save("ds", "deepseek", None)
        })
        .expect("change the model");
    assert_eq!(
        server.model_profiles().profiles[0].vision,
        Some(true),
        "not mentioning vision must not wipe it out"
    );
}

#[tokio::test]
async fn capabilities_are_validated_before_credentials_and_survive_profile_patches() {
    let (server, home) = server().await;
    let mut bad = save("oa", "openai", Some("gpt-6.1-sol"));
    bad.api_key = Some("synthetic-test-secret".into());
    bad.capabilities = Some(jeeves_protocol::ModelCapabilities {
        reasoning_effort: Some("none".into()),
        ..Default::default()
    });
    assert!(server.model_save(bad).is_err());
    assert!(!home.path().join("secrets.json").exists());
    assert!(server.model_profiles().profiles.is_empty());
    let mut params = save("oa", "openai", Some("gpt-6.1-sol"));
    params.capabilities = Some(jeeves_protocol::ModelCapabilities {
        tools: Some(false),
        context_window: Some(100_000),
        reasoning_effort: Some("high".into()),
    });
    server.model_save(params).unwrap();
    let expected = server.model_profiles().profiles[0].capabilities.clone();
    server
        .model_save(ModelSaveParams {
            provider: None,
            ..save("oa", "", None)
        })
        .unwrap();
    assert_eq!(server.model_profiles().profiles[0].capabilities, expected);
    assert_eq!(
        jeeves_config::models::load_models(home.path()).profiles[0].capabilities,
        expected
    );
    server
        .model_save(ModelSaveParams {
            capabilities: Some(Default::default()),
            ..save("oa", "openai", None)
        })
        .unwrap();
    assert_eq!(
        server.model_profiles().profiles[0].capabilities,
        Some(Default::default())
    );
}

#[tokio::test]
async fn review_unknown_provider_with_endpoint_returns_an_actionable_error() {
    let (server, _dir) = server().await;
    let err = server
        .model_save(ModelSaveParams {
            base_url: Some("http://localhost:9000/v1".into()),
            ..save("local", "unlisted", Some("demo"))
        })
        .unwrap_err();
    assert!(err.contains("unlisted") && err.contains("models-catalog.json"));
    assert!(server.model_profiles().profiles.is_empty());
}

#[tokio::test]
async fn review_persistence_failure_keeps_the_last_saved_profile() {
    let (server, dir) = server().await;
    server
        .model_save(save("local", "ollama", Some("first")))
        .unwrap();
    let path = cfg::models_path(dir.path());
    std::fs::rename(&path, dir.path().join("original")).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        server
            .model_save(save("local", "ollama", Some("second")))
            .is_err()
    );
    assert_eq!(
        server.model_profiles().profiles[0].model.as_deref(),
        Some("first")
    );
}

#[tokio::test]
async fn review_removed_active_profile_releases_its_runtime_configuration() {
    let (server, _dir) = server().await;
    let original = server.settings();
    server
        .model_save(save("local", "ollama", Some("fixture-only")))
        .unwrap();
    server
        .model_remove(ModelRemoveParams { id: "local".into() })
        .unwrap();
    let now = server.settings();
    assert_eq!(
        (now.provider, now.model),
        (original.provider, original.model)
    );
}

#[tokio::test]
async fn editing_runtime_settings_keeps_the_custom_catalog_and_preserves_damaged_settings() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models-catalog.json"), r#"[{"id":"fixture-box","wire":"openai","model":"local","base_url":"http://127.0.0.1:9/v1"}]"#).unwrap();
    let server = Server::boot(
        jeeves_state::open(&dir.path().join("state.db"))
            .await
            .unwrap(),
        jeeves_config::Settings {
            home: dir.path().into(),
            ..Default::default()
        },
    )
    .await;
    server
        .model_save(ModelSaveParams {
            key_env: None,
            ..save("box", "fixture-box", None)
        })
        .unwrap();
    let patch = jeeves_protocol::SettingsPatch {
        history_turns: Some(7),
        ..Default::default()
    };
    server.apply_config_patch(&patch).unwrap();
    assert_eq!(server.resolved().unwrap().provider_id, "fixture-box");
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "damaged fixture").unwrap();
    assert!(server.apply_config_patch(&patch).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "damaged fixture");
}

#[tokio::test]
async fn removing_a_profile_can_be_retried_after_credential_cleanup_fails() {
    let (server, dir) = server().await;
    server
        .secrets
        .lock()
        .unwrap()
        .set_backend(cfg::SecretBackend::File);
    let mut profile = save("review-fixture", "openai", None);
    profile.api_key = Some("synthetic-value".into());
    server.model_save(profile).unwrap();
    let path = dir.path().join("secrets.json");
    let original = std::fs::read(&path).unwrap();
    std::fs::write(&path, "{broken").unwrap();
    let remove = || {
        server.model_remove(ModelRemoveParams {
            id: "review-fixture".into(),
        })
    };
    assert!(remove().is_err());
    assert_eq!(server.model_profiles().profiles.len(), 1);
    std::fs::write(&path, original).unwrap();
    assert!(remove().unwrap().removed);
    assert!(server.model_profiles().profiles.is_empty());
    assert!(
        !std::fs::read_to_string(path)
            .unwrap()
            .contains("synthetic-value")
    );
}
