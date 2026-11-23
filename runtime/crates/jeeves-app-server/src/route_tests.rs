//! Pure-function tests for route decisions — tier selection and the fallback chain.
//!
//! Integration with Server (real resolution, fallback paths) is covered by eval's mock-provider scenarios;
//! what is pinned here is the rules themselves.

use jeeves_config::routes::{RouteFile, RouteMode};
use std::collections::HashMap;

use super::{pick_tier, profile_for_tier};

fn routes_with(entries: &[(&str, &str)]) -> RouteFile {
    let mut routes = RouteFile {
        mode: RouteMode::Auto,
        routes: HashMap::new(),
        fallback: Vec::new(),
    };
    for (tier, id) in entries {
        routes.routes.insert(tier.to_string(), id.to_string());
    }
    routes
}

#[test]
fn tier_is_reasoning_past_half_the_window() {
    assert_eq!(pick_tier(1_000, 32_768), "chat");
    assert_eq!(pick_tier(16_383, 32_768), "chat");
    assert_eq!(pick_tier(16_384, 32_768), "reasoning");
    assert_eq!(
        pick_tier(499_999, 1_000_000),
        "chat",
        "a big window only switches once the message is really big"
    );
    assert_eq!(pick_tier(600_000, 1_000_000), "reasoning");
}

#[test]
fn tier_lookup_falls_back_through_the_chain() {
    let empty = routes_with(&[]);
    assert_eq!(
        profile_for_tier(&empty, "reasoning", Some("ds")).as_deref(),
        Some("ds"),
        "a missing reasoning tier → chat → active"
    );
    assert_eq!(
        profile_for_tier(&empty, "fast", Some("ds")).as_deref(),
        Some("ds")
    );
    assert_eq!(
        profile_for_tier(&empty, "chat", None),
        None,
        "all missing = None"
    );

    let full = routes_with(&[("reasoning", "ds-r1"), ("chat", "ds")]);
    assert_eq!(
        profile_for_tier(&full, "reasoning", Some("oa")).as_deref(),
        Some("ds-r1")
    );
    assert_eq!(
        profile_for_tier(&full, "fast", Some("oa")).as_deref(),
        Some("ds"),
        "a missing fast tier → the chat set"
    );
    assert_eq!(
        profile_for_tier(&full, "chat", Some("oa")).as_deref(),
        Some("ds")
    );

    let only_reasoning = routes_with(&[("reasoning", "ds-r1")]);
    assert_eq!(
        profile_for_tier(&only_reasoning, "fast", Some("oa")).as_deref(),
        Some("oa")
    );
}

#[test]
fn a_declared_tier_is_validated_not_guessed() {
    use super::clean_tier;

    assert_eq!(clean_tier(None).expect("the default is legal"), None);
    assert_eq!(
        clean_tier(Some("   ")).expect("blank means undeclared"),
        None
    );
    assert_eq!(
        clean_tier(Some(" reasoning "))
            .expect("strip the whitespace")
            .as_deref(),
        Some("reasoning")
    );
    for tier in ["fast", "chat", "reasoning"] {
        assert_eq!(
            clean_tier(Some(tier))
                .expect("all three tiers are legal")
                .as_deref(),
            Some(tier)
        );
    }
    let error = clean_tier(Some("reasoining")).expect_err("a misspelling must fail");
    assert!(
        error.message.contains("fast") && error.message.contains("reasoining"),
        "the error has to say the allowed values and the one received: {}",
        error.message
    );
}

#[test]
fn classification_only_runs_in_the_ambiguous_band() {
    use super::needs_classify;

    assert!(
        !needs_classify(1_999, 32_768),
        "a short message is not classified"
    );
    assert!(needs_classify(2_000, 32_768));
    assert!(needs_classify(16_383, 32_768));
    assert!(
        !needs_classify(16_384, 32_768),
        "more than half already judged reasoning by L0"
    );
}

#[test]
fn tier_reply_parsing_fails_open() {
    use super::parse_tier_reply;

    assert_eq!(parse_tier_reply("reasoning"), Some("reasoning"));
    assert_eq!(
        parse_tier_reply("Reasoning\n"),
        Some("reasoning"),
        "case and newlines do not matter"
    );
    assert_eq!(parse_tier_reply("chat"), None);
    assert_eq!(parse_tier_reply(""), None);
    assert_eq!(
        parse_tier_reply("this looks like a reasoning task, because…"),
        None,
        "extra explanation does not count — one word only"
    );
}
