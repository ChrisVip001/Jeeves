use super::*;
use jeeves_config::models::{ModelLine, ModelOrigin, ModelProfile, SecretStore};

const KEY: &str = "sk-test-SECRET-123456";

fn profile(id: &str, source: Option<&str>) -> ModelProfile {
    ModelProfile {
        id: id.into(),
        provider: "deepseek".into(),
        model: None,
        small_model: None,
        base_url: None,
        credential_slot: Default::default(),
        key_source: source.map(String::from),
        vision: None,
        capabilities: None,
        origin: ModelOrigin::default(),
        line: ModelLine::language(),
    }
}

fn run(
    home: &Path,
    models: &ModelsFile,
    env: &dyn Fn(&str) -> Option<String>,
    now: i64,
    connected: &[String],
    cards: &Cards,
) -> Vec<CapabilityCard> {
    let mut vault = SecretStore::memory();
    let mut e = ProbeEnv {
        home,
        now,
        env,
        vault: &mut vault,
        models,
        connected_mcp: connected,
        missing_scope_mcp: &[],
        mcp_extra: None,
    };
    cards.refresh(&mut e)
}

#[test]
fn missing_env_key_is_unauthorized_with_pointer_and_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let models = ModelsFile {
        active: None,
        profiles: vec![
            profile("ds", Some("env:DS_KEY")),
            profile("ok", Some("env:OK_KEY")),
        ],
    };
    let env = |n: &str| (n == "OK_KEY").then(|| KEY.to_string());
    let cards = run(dir.path(), &models, &env, 1000, &[], &Cards::default());
    let ds = cards.iter().find(|c| c.id == "model:ds").unwrap();
    assert_eq!(ds.state, CapabilityState::Unauthorized);
    assert_eq!(ds.secret_pointer.as_deref(), Some("env:DS_KEY"));
    let ok = cards.iter().find(|c| c.id == "model:ok").unwrap();
    assert_eq!(ok.state, CapabilityState::Ready);
    let json = serde_json::to_string(&cards).unwrap();
    assert!(!json.contains(KEY), "a card must never carry the key");
}

#[test]
fn expired_mcp_oauth_token_reports_expired() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mcp.json"),
        r#"{"servers":[{"name":"gh","url":"https://x.example/mcp","oauth":true},
                       {"name":"bare","url":"https://y.example/mcp","oauth":true}]}"#,
    )
    .unwrap();
    let store = jeeves_mcp::oauth::TokenStore::new(dir.path(), "gh");
    store
        .save_tokens(jeeves_mcp::oauth::StoredTokens {
            access_token: KEY.into(),
            refresh_token: None,
            expires_at: Some(1),
        })
        .unwrap();
    let models = ModelsFile::default();
    let connected = vec!["gh".to_string(), "bare".to_string()];
    let cards = run(
        dir.path(),
        &models,
        &|_| None,
        1000,
        &connected,
        &Cards::default(),
    );
    let gh = cards.iter().find(|c| c.id == "mcp:gh").unwrap();
    assert_eq!(gh.state, CapabilityState::Expired);
    let bare = cards.iter().find(|c| c.id == "mcp:bare").unwrap();
    assert_eq!(bare.state, CapabilityState::Unauthorized);
    assert!(!serde_json::to_string(&cards).unwrap().contains(KEY));
}

#[test]
fn unconnected_mcp_server_is_offline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mcp.json"),
        r#"{"servers":[{"name":"local","command":"nothing"}]}"#,
    )
    .unwrap();
    let cards = run(
        dir.path(),
        &ModelsFile::default(),
        &|_| None,
        5,
        &[],
        &Cards::default(),
    );
    assert_eq!(cards[0].state, CapabilityState::Offline);
}

#[test]
fn feishu_without_secret_is_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("connectors.json"),
        r#"{"feishu":{"app_id":"cli_a","app_secret":"env:FS_SECRET"}}"#,
    )
    .unwrap();
    let cards = run(
        dir.path(),
        &ModelsFile::default(),
        &|_| None,
        5,
        &[],
        &Cards::default(),
    );
    let f = cards.iter().find(|c| c.id == "connector:feishu").unwrap();
    assert_eq!(f.state, CapabilityState::Unauthorized);
    assert_eq!(f.secret_pointer.as_deref(), Some("env:FS_SECRET"));
    let env = |n: &str| (n == "FS_SECRET").then(|| KEY.to_string());
    let cards = run(
        dir.path(),
        &ModelsFile::default(),
        &env,
        5,
        &[],
        &Cards::default(),
    );
    assert_eq!(cards[0].state, CapabilityState::BlockedByPolicy);
}

#[test]
fn credential_pointer_reports_actual_storage() {
    let dir = tempfile::tempdir().unwrap();
    let mut vault = SecretStore::file_only();
    vault.put(dir.path(), "test-account", KEY).unwrap();
    let models = ModelsFile {
        active: None,
        profiles: vec![profile("test-account", Some("keychain"))],
    };
    let mut env = ProbeEnv {
        home: dir.path(),
        now: 5,
        env: &|_| None,
        vault: &mut vault,
        models: &models,
        connected_mcp: &[],
        missing_scope_mcp: &[],
        mcp_extra: None,
    };
    let cards = Cards::default().refresh(&mut env);
    assert_eq!(cards[0].secret_pointer.as_deref(), Some("file"));
    assert_eq!(cards[0].state, CapabilityState::Ready);
    assert!(!serde_json::to_string(&cards).unwrap().contains(KEY));
}

#[test]
fn explicit_remote_scope_failure_is_not_confused_with_offline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("mcp.json"),
        r#"{"servers":[{"name":"scope","url":"https://example.test/mcp"}]}"#,
    )
    .unwrap();
    let mut vault = SecretStore::memory();
    let models = ModelsFile::default();
    let missing = vec!["scope".to_string()];
    let mut env = ProbeEnv {
        home: dir.path(),
        now: 5,
        env: &|_| None,
        vault: &mut vault,
        models: &models,
        connected_mcp: &[],
        missing_scope_mcp: &missing,
        mcp_extra: None,
    };
    let cards = Cards::default().refresh(&mut env);
    assert_eq!(cards[0].state, CapabilityState::MissingScope);
}

#[test]
fn calendar_and_email_report_missing_secrets_without_leaking_values() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("connectors.json"),
        r#"{
          "calendar":{"url":"https://example.com/cal/","username":"a","password":"env:CAL_PASS"},
          "email":{
            "username":"alice@example.com","password":"env:MAIL_PASS",
            "from":"Alice <alice@example.com>",
            "imap":{"host":"imap.example.com","port":993,"security":"tls"},
            "smtp":{"host":"smtp.example.com","port":587,"security":"starttls"}
          }
        }"#,
    )
    .unwrap();
    let cards = run(
        dir.path(),
        &ModelsFile::default(),
        &|_| None,
        5,
        &[],
        &Cards::default(),
    );
    let calendar = cards.iter().find(|c| c.id == "connector:calendar").unwrap();
    let email = cards.iter().find(|c| c.id == "connector:email").unwrap();
    assert_eq!(calendar.state, CapabilityState::Unauthorized);
    assert_eq!(email.state, CapabilityState::Unauthorized);
    assert_eq!(calendar.secret_pointer.as_deref(), Some("env:CAL_PASS"));
    assert_eq!(email.secret_pointer.as_deref(), Some("env:MAIL_PASS"));
    assert!(!serde_json::to_string(&cards).unwrap().contains(KEY));
}

#[test]
fn cards_are_cached_for_sixty_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let models = ModelsFile {
        active: None,
        profiles: vec![profile("ds", Some("env:DS_KEY"))],
    };
    let cache = Cards::default();
    let first = run(dir.path(), &models, &|_| None, 1000, &[], &cache);
    assert_eq!(first[0].state, CapabilityState::Unauthorized);
    let env = |_: &str| Some(KEY.to_string());
    let within = run(dir.path(), &models, &env, 1030, &[], &cache);
    assert_eq!(
        within[0].state,
        CapabilityState::Unauthorized,
        "served from cache"
    );
    let after = run(dir.path(), &models, &env, 1061, &[], &cache);
    assert_eq!(after[0].state, CapabilityState::Ready);
}
