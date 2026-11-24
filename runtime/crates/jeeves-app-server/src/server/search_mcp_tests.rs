use super::*;

#[test]
fn search_and_fetch_map_to_read_only_tools() {
    let (tool, args) = request(&json!({"query":"Tokio timeout docs"})).unwrap();
    assert_eq!(tool, "web_search");
    assert_eq!(args["search_queries"], json!(["Tokio timeout docs"]));
    let (tool, args) = request(&json!({"url":"https://tokio.rs/"})).unwrap();
    assert_eq!(tool, "web_fetch");
    assert_eq!(args["urls"], json!(["https://tokio.rs/"]));
    assert!(request(&json!({"query":" "})).is_err());
}

#[test]
fn malformed_empty_error_and_wrong_fetch_results_fail_closed() {
    for text in ["not JSON", "{}", r#"{"error":"quota"}"#] {
        assert!(render(text, &json!({"query":"Tokio"})).is_err(), "{text}");
    }
    for text in [
        r#"{"results":[]}"#,
        r#"{"results":[{"url":"javascript:bad","excerpts":["bad"]}]}"#,
        r#"{"results":[{"url":"https://tokio.rs/","excerpts":[]}]}"#,
    ] {
        assert!(render(text, &json!({"query":"Tokio"})).unwrap().is_none());
    }
    assert!(
        render(
            r#"{"results":[{"url":"https://other.test/","full_content":"wrong page"}]}"#,
            &json!({"url":"https://tokio.rs/"})
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn sources_keep_provenance_scope_and_bounded_excerpts() {
    let body = json!({"results":[
        {"url":"https://wrong.test/", "excerpts":["irrelevant"]},
        {"url":"https://tokio.rs/tokio/tutorial", "title":"Tutorial", "publish_date":"2026-11-15", "excerpts":["x".repeat(5000)]},
        {"url":"https://tokio.rs/tokio/tutorial#duplicate", "excerpts":["duplicate"]}
    ]});
    let text = render(
        &body.to_string(),
        &json!({"query":"site:tokio.rs tokio tutorial", "max_results":5}),
    )
    .unwrap()
    .unwrap();
    assert!(text.contains("Parallel MCP"));
    assert!(text.contains("2026-11-15"));
    assert!(text.contains("truncated"));
    assert!(!text.contains("wrong.test"));
    assert!(!text.contains("Source 2"));
    assert!(text.len() < 3000);
}

#[tokio::test]
async fn cooldown_skips_network() {
    let state = Mutex::new(State {
        client: None,
        retry_at: Some(Instant::now() + COOLDOWN),
    });
    let error = execute(&state, Path::new("/unused"), json!({"query":"anything"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("cooling down"));
}

#[tokio::test]
#[ignore = "Calls the official keyless Parallel MCP; no model or paid credentials"]
async fn live_parallel_search_and_fetch() {
    let home = tempfile::tempdir().unwrap();
    let state = Mutex::new(State::default());
    let text = execute(
        &state,
        home.path(),
        json!({"query":"site:tokio.rs tokio tutorial", "max_results":3}),
    )
    .await
    .unwrap();
    assert!(text.contains("https://tokio.rs/"), "{text}");
    let fetched = execute(
        &state,
        home.path(),
        json!({"url":"https://tokio.rs/tokio/tutorial"}),
    )
    .await
    .unwrap();
    assert!(fetched.to_lowercase().contains("tutorial"), "{fetched}");
    eprintln!("Live Parallel MCP returned scoped sources and readable tutorial content");
}

#[test]
fn fetch_requires_readable_full_content_instead_of_search_excerpts() {
    let source = "https://docs.example/rules";
    for content in [Value::Null, json!(" \n ")] {
        let body = json!({"results":[{
            "url": source,
            "full_content": content,
            "excerpts":["A brief lead is not the full source"]
        }]});
        assert!(
            render(&body.to_string(), &json!({"url":source}))
                .unwrap()
                .is_none()
        );
        assert!(
            render(&body.to_string(), &json!({"query":"new rules"}))
                .unwrap()
                .unwrap()
                .contains("A brief lead")
        );
    }
}

#[test]
fn parallel_search_deduplicates_tracking_links_but_preserves_distinct_revisions() {
    let first = "https://museum.example/hours?id=7&utm_source=directory";
    let current = "https://museum.example/hours?id=8";
    let body = json!({"results":[
        {"url":first, "excerpts":["旧版营业时间"]},
        {"url":"https://museum.example/hours?utm_content=ad&id=7", "excerpts":["旧公告重复"]},
        {"url":current, "excerpts":["新版营业时间"]},
    ]});
    let text = render(
        &body.to_string(),
        &json!({"query":"site:museum.example 营业时间","max_results":2}),
    )
    .unwrap()
    .unwrap();
    assert!(text.contains(first), "{text}");
    assert!(text.contains(current), "{text}");
    assert!(!text.contains("旧公告重复"), "{text}");
}

#[test]
fn exact_fetch_does_not_treat_tracking_normalization_as_page_identity() {
    let body = json!({"results":[
        {"url":"https://museum.example/hours?id=7&utm_source=directory","full_content":"Unmatched"}
    ]});
    assert!(
        render(
            &body.to_string(),
            &json!({"url":"https://museum.example/hours?id=7"})
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn repeated_official_scope_cannot_let_an_old_page_take_the_only_slot() {
    let body = json!({"results":[
        {"url":"https://library.example/notices-old/hours","excerpts":["去年营业时间"]},
        {"url":"https://directory.example/notices/hours","excerpts":["第三方转述"]},
        {"url":"https://library.example/notices/hours","excerpts":["官方最新营业时间"]}
    ]});
    let text = render(
        &body.to_string(),
        &json!({"query":"site:library.example/notices 营业时间 SITE:Library.Example/notices/","max_results":1}),
    )
    .unwrap()
    .unwrap();
    assert!(text.contains("官方最新营业时间"), "{text}");
}
