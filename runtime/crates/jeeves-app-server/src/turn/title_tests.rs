use std::sync::Arc;

use jeeves_provider::Resolved;
use jeeves_provider::mock::Mock;

use super::{clean, propose, wants_title};

fn resolved(mock: Mock) -> Resolved {
    Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(mock),
        model: "main".to_string(),
        small_model: "small".to_string(),
    }
}

#[test]
fn a_title_is_trimmed_of_quotes_prefixes_and_trailing_punctuation() {
    assert_eq!(
        clean("\"Title: Fix  login button.\"").as_deref(),
        Some("Fix login button")
    );
    assert_eq!(clean("「修复登录按钮」").as_deref(), Some("修复登录按钮"));
    assert_eq!(clean("标题：周报整理。").as_deref(), Some("周报整理"));
}

#[test]
fn an_answer_shaped_reply_is_refused_not_truncated() {
    assert_eq!(clean("   "), None);
    assert_eq!(
        clean("Sure, I can help you fix the login button on your mobile site right away today"),
        None
    );
    assert_eq!(
        clean("可以的，我来帮你看看这个登录按钮在手机上为什么没有反应，然后给出修复方法"),
        None
    );
    assert_eq!(clean("first line\nsecond line"), None);
    assert_eq!(
        clean("Node.js 内存泄漏排查").as_deref(),
        Some("Node.js 内存泄漏排查")
    );
}

#[test]
fn internal_sessions_are_not_titled() {
    assert!(wants_title("b1f2c3"));
    assert!(wants_title("jeeves-main"));
    assert!(!wants_title("schedule:morning-news"));
    assert!(!wants_title("task-control:abc"));
}

#[tokio::test]
async fn the_small_model_is_asked_once_and_its_json_title_is_used() {
    let mock = Mock::new(vec![Mock::text(
        "```json\n{\"title\": \"Postgres pool exhaustion\"}\n```",
    )]);
    let model = resolved(mock);
    let title = propose(
        &model,
        "my postgres pool keeps running out, here is the log ...",
    )
    .await
    .expect("a title");
    assert_eq!(title, "Postgres pool exhaustion");
}

#[tokio::test]
async fn a_reply_without_a_title_field_is_an_error_not_a_title() {
    let model = resolved(Mock::new(vec![Mock::text(
        "Here is how to fix your pool: raise max_connections.",
    )]));
    let why = propose(&model, "pool keeps running out")
        .await
        .expect_err("no title");
    assert!(why.contains("title"), "{why}");
}
