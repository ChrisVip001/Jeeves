//! Config parsing. The allowlist is the one place where a mistake means "the agent messaged
//! a stranger", so a typo in its key must fail loudly.

use super::*;
use tempfile::tempdir;

#[test]
fn a_missing_file_means_no_connectors_and_a_broken_one_is_loud() {
    let dir = tempdir().expect("temp dir");
    let quiet = load(dir.path()).expect("a missing file is not an error");
    assert!(quiet.feishu.is_none());

    std::fs::write(dir.path().join("connectors.json"), "{ not json").expect("write");
    let error = load(dir.path()).expect_err("a bad file has to fail loudly");
    assert!(error.to_string().contains("connectors.json"), "{error}");
}

#[test]
fn a_typo_in_a_key_is_rejected_instead_of_silently_meaning_nobody() {
    let dir = tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("connectors.json"),
        r#"{"feishu": {
            "app_id": "cli_a", "app_secret": "env:X",
            "allow_recipents": ["ou_1"]
        }}"#,
    )
    .expect("write");
    let error = load(dir.path()).expect_err("a misspelt field name must be refused");
    assert!(error.to_string().contains("allow_recipents"), "{error}");
}

#[test]
fn live_is_off_unless_the_file_says_so() {
    let config: ConnectorConfig =
        serde_json::from_str(r#"{"feishu": {"app_id": "cli_a", "app_secret": "env:X"}}"#)
            .expect("parse");
    let feishu = config.feishu.expect("there is a feishu section");
    assert!(!feishu.live, "the default can only be a draft");
    assert!(
        feishu.allow_recipients.is_empty(),
        "by default nobody is allowed"
    );
}
