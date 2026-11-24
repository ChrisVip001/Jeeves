//! Behavior edges of `fetch_url`. Network cases all hit a **local** HTTP server
//! (only `allow_private=true` may reach 127.0.0.1; the tool itself always passes SSRF checks),
//! never the real internet — CI cannot depend on the outside world.

use super::web::{check_url, execute_fetch, fetch_url, html_to_text, is_disallowed_ip};
use crate::{ToolCtx, ToolRegistry};
use serde_json::json;

pub(crate) async fn ctx() -> ToolCtx {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        activate: None,
        cwd: None,
        approval: None,
        jobs: None,
        turn_limits: std::sync::Arc::new(crate::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home,
        on_delta: None,
    }
}

#[test]
fn disallowed_ips_cover_loopback_private_and_metadata() {
    for bad in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.9",
        "192.168.1.1",
        "169.254.169.254",
        "0.0.0.0",
        "192.0.2.7",
        "::1",
        "fe80::1",
        "fd00::1",
    ] {
        let ip: std::net::IpAddr = bad.parse().unwrap();
        assert!(is_disallowed_ip(ip), "{bad} should be refused");
    }
    for good in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
        let ip: std::net::IpAddr = good.parse().unwrap();
        assert!(!is_disallowed_ip(ip), "{good} should be allowed");
    }
}

#[tokio::test]
async fn non_http_schemes_are_refused() {
    let url = reqwest::Url::parse("file:///etc/passwd").unwrap();
    let err = check_url(&url, true).await.unwrap_err().to_string();
    assert!(err.contains("only http/https is supported"), "{err}");
}

#[test]
fn html_to_text_strips_scripts_and_decodes_entities() {
    let html = "<html><head><title>the title</title><style>body{color:red}</style></head>\
                <body><p>the first paragraph &amp; a symbol</p><script>evil()</script>\
                <p>numeric entity &#28450; char</p><br/>/*comment<!--hidden-->*/tail</body></html>";
    let text = html_to_text(html);
    assert!(text.contains("title"), "{text}");
    assert!(text.contains("the first paragraph & a symbol"), "{text}");
    assert!(
        text.contains("numeric entity 漢 char"),
        "a numeric entity for a CJK character decodes: {text}"
    );
    assert!(text.contains("tail"), "{text}");
    assert!(
        !text.contains("evil"),
        "script bodies must not survive: {text}"
    );
    assert!(
        !text.contains("color:red"),
        "style bodies must not survive: {text}"
    );
    assert!(
        !text.contains("hidden"),
        "comments must not survive: {text}"
    );
    let lines: Vec<&str> = text.lines().collect();
    for pair in lines.windows(2) {
        assert!(
            !(pair[0].trim().is_empty() && pair[1].trim().is_empty()),
            "consecutive blank lines were not folded: {text:?}"
        );
    }
    assert!(
        lines.iter().all(|l| *l == l.trim()),
        "whitespace left at the line edges: {text:?}"
    );
}

#[test]
fn unknown_entities_pass_through() {
    assert!(html_to_text("<p>A &notanentity; B</p>").contains("&notanentity;"));
}

fn spawn_server(routes: Vec<(&'static str, u16, &'static str, String, &'static str)>) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..16 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]);
            let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
            let Some((status, ct, body, extra)) = routes
                .iter()
                .find(|(p, ..)| *p == path)
                .map(|(_, s, c, b, e)| (*s, *c, b, *e))
            else {
                continue;
            };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                status,
                if status < 300 { "OK" } else { "See Other" },
                ct,
                body.len(),
                extra,
                body,
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn follows_redirects_and_wraps_the_body_as_external_data() {
    let base = spawn_server(vec![
        (
            "/start",
            302,
            "text/html",
            String::new(),
            "Location: /final\r\n",
        ),
        (
            "/final",
            200,
            "text/html",
            "<html><body><h1>the body title</h1><p>hello</p><script>alert(1)</script></body></html>"
                .into(),
            "",
        ),
    ]);
    let out = execute_fetch(&format!("{base}/start"), true).await.unwrap();
    assert!(
        out.starts_with("Below is the body of an external web page"),
        "{out}"
    );
    assert!(out.contains("the body title"), "{out}");
    assert!(out.contains("hello"), "{out}");
    assert!(!out.contains("alert"), "scripts must not survive: {out}");
}

#[tokio::test]
async fn a_redirect_to_a_private_target_is_refused() {
    let url = reqwest::Url::parse("http://10.0.0.1/x").unwrap();
    let err = check_url(&url, false).await.unwrap_err().to_string();
    assert!(err.contains("internal or reserved address"), "{err}");
}

#[tokio::test]
async fn redirect_loops_stop_at_the_cap() {
    let base = spawn_server(vec![(
        "/loop",
        302,
        "text/plain",
        String::new(),
        "Location: /loop\r\n",
    )]);
    let err = execute_fetch(&format!("{base}/loop"), true)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("redirected more than"), "{err}");
}

#[tokio::test]
async fn http_errors_are_honest() {
    let base = spawn_server(vec![("/gone", 404, "text/plain", "gone".into(), "")]);
    let err = execute_fetch(&format!("{base}/gone"), true)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("404"), "{err}");
}

#[tokio::test]
async fn the_output_is_capped_with_an_honest_marker() {
    let base = spawn_server(vec![(
        "/long",
        200,
        "text/plain",
        "char".repeat(60_000),
        "",
    )]);
    let out = execute_fetch(&format!("{base}/long"), true).await.unwrap();
    assert!(out.contains("truncated at 50000"), "{out}");
    assert!(
        out.chars().count() < 51_000,
        "still stuffing more in past 50k: {}",
        out.chars().count()
    );
}

#[tokio::test]
async fn the_tool_refuses_loopback_without_the_test_knob() {
    let mut registry = ToolRegistry::new();
    registry.register(fetch_url());
    let out = registry
        .execute(
            ctx().await,
            "fetch_url",
            json!({"url": "http://127.0.0.1:9/"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("internal or reserved address"), "{out}");
}

use super::web::{Backend, TurnLimits, plan_backends, search_web};
use std::sync::atomic::Ordering;

#[test]
fn backend_plan_prefers_keyed_and_uses_ddg_lite_before_bing() {
    let plan = plan_backends(Some("t".into()), None, true, true);
    assert_eq!(
        plan.len(),
        1,
        "with a key, the keyless backend stays out of the plan"
    );
    assert!(matches!(plan[0], Backend::Tavily(_)));

    let plan = plan_backends(None, Some("b".into()), true, true);
    assert!(matches!(plan[0], Backend::Brave(_)));

    let plan = plan_backends(None, None, true, true);
    assert_eq!(plan.len(), 2, "with no key both keyless backends get tried");
    assert!(
        matches!(plan[0], Backend::DuckDuckGo),
        "DDG Lite is the primary keyless endpoint"
    );
    assert!(
        matches!(plan[1], Backend::Bing),
        "Bing is retained as a fallback"
    );

    assert!(
        matches!(
            plan_backends(None, None, false, true)[0],
            Backend::DuckDuckGo
        ),
        "DDG Lite is the primary keyless endpoint"
    );
    assert!(matches!(
        plan_backends(None, None, true, false)[0],
        Backend::Bing
    ));
    assert!(
        plan_backends(None, None, false, false).is_empty(),
        "all off means no backend — search_ready has to have kept it out of registration"
    );
}

#[test]
fn bing_result_page_parses_into_hits() {
    let page = r#"<html><body>
      <li class="b_algo" data-id="1"><h2><a href="https://example.com/a">CodeBuddy &amp; IDE</a></h2>
        <div class="b_caption"><p>an introduction to an <b>IDE</b></p></div></li>
      <li class="b_algo"><h2><a href="https://example.com/b">the second entry</a></h2>
        <div class="b_caption"><p>another result</p></div></li>
    </body></html>"#;
    let hits = super::web::parse_bing(page, 5);
    assert_eq!(hits.len(), 2);
    assert_eq!(
        hits[0].0, "CodeBuddy & IDE",
        "HTML entities have to be decoded"
    );
    assert_eq!(
        hits[0].1, "an introduction to an IDE",
        "the tags have to go"
    );
    assert_eq!(hits[0].2, "https://example.com/a");
    assert_eq!(hits[1].0, "the second entry");
    assert_eq!(
        super::web::parse_bing(page, 1).len(),
        1,
        "max_results has to take effect"
    );
}

#[test]
fn brave_results_parse_into_hits() {
    let data = json!({"web": {"results": [
        {"title": "T", "description": "D", "url": "https://example.com/x"}
    ]}});
    let hits = super::web::parse_brave(&data).unwrap();
    assert_eq!(
        hits,
        vec![(
            "T".to_string(),
            "D".to_string(),
            "https://example.com/x".to_string()
        )]
    );
    assert!(super::web::parse_brave(&json!({})).unwrap().is_empty());
}

#[tokio::test]
async fn search_cap_stops_the_turn_before_any_network() {
    let mut registry = ToolRegistry::new();
    registry.register(search_web());
    let c = ctx().await;
    c.turn_limits
        .used
        .store(c.turn_limits.search_max, Ordering::Relaxed);
    let out = registry
        .execute(c, "search_web", json!({"query": "anything"}))
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("limit"), "{out}");
}

#[test]
fn turn_limits_start_empty() {
    let limits = TurnLimits::default();
    assert_eq!(limits.used.load(Ordering::Relaxed), 0);
    assert_eq!(limits.search_max, 20);
    assert_eq!(
        TurnLimits::new(0).search_max,
        1,
        "0 is clamped to the lower bound; the budget is never negative"
    );
    assert_eq!(
        TurnLimits::new(99_999).search_max,
        200,
        "clamped to the upper bound"
    );
}

#[test]
fn an_ampersand_before_a_wide_character_does_not_panic() {
    let text = "&中文中文中文中文中文中文";
    let out = html_to_text(text);
    assert!(
        out.contains('&'),
        "an entity it cannot read stays as it is: {out}"
    );
    assert!(out.contains("中文"), "the body must not be eaten: {out}");

    assert_eq!(html_to_text("&amp;text").trim(), "&text");
    assert_eq!(html_to_text("&#20013;").trim(), "中");
}

#[test]
fn bing_results_do_not_borrow_the_next_snippet_or_expose_tracking_links() {
    let page = r#"<li data-id="1" class="b_algo"><h2><a href="/ck/a?u=a1aHR0cHM6Ly9kb2NzLnJzL3Rva2lvLw&amp;ntb=1">Tokio docs</a></h2></li>
    <li class="b_algo"><h2><a href="https://example.com/second">Second</a></h2><p>Only the second snippet.</p></li>"#;
    let hits = super::web::parse_bing(page, 5);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].1, "");
    assert_eq!(hits[0].2, "https://docs.rs/tokio/");
    assert_eq!(hits[1].1, "Only the second snippet.");
}

#[test]
fn duckduckgo_lite_and_html_keep_links_and_snippets_in_their_result() {
    let page = r#"<tr><td><a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio%2F%3Fpage%3D1%26lang%3Den&amp;rut=tracking" class='result-link'>Tokio &amp; Rust</a></td></tr>
    <tr><td class='result-snippet'>A <b>timeout</b> future.</td></tr>
    <a class='result-link' href='https://docs.rs/tokio/?page=1&amp;lang=en#duplicate'>Duplicate</a>
    <a href="https://example.com/no-snippet" class="result__a">No snippet</a>
    <a class="result__a" href="https://example.com/last">Last</a><a class="result__snippet">Last &#x897f;&#X6e56; snippet</a>"#;
    let hits = super::web::parse_duckduckgo(page, 5);
    assert_eq!(hits.len(), 3);
    assert_eq!(
        hits[0],
        (
            "Tokio & Rust".into(),
            "A timeout future.".into(),
            "https://docs.rs/tokio/?page=1&lang=en".into()
        )
    );
    assert_eq!(hits[1].1, "");
    assert_eq!(hits[2].1, "Last 西湖 snippet");
    assert_eq!(super::web::parse_duckduckgo(page, 1).len(), 1);
    assert!(
        super::web::parse_duckduckgo("<form id='challenge-form'>No results here</form>", 5)
            .is_empty()
    );
}

#[test]
fn unusable_and_duplicate_hits_do_not_consume_the_result_budget() {
    let hit = |title: &str, url: &str| (title.into(), "text ".repeat(100), url.into());
    let hits = super::web::clean_hits(
        vec![
            hit("Bad", "javascript:alert(1)"),
            hit(" ", "https://example.com/empty"),
            hit("Tokio <T>", "https://example.com/a#first"),
            hit("Duplicate", "https://example.com/a#second"),
            hit("Other", "https://example.com/b"),
        ],
        2,
    );
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].0, "Tokio <T>");
    assert_eq!(hits[0].2, "https://example.com/a#first");
    assert!(hits[0].1.chars().count() <= 401);
    assert_eq!(hits[1].0, "Other");
}

#[tokio::test]
#[ignore = "Explicit public-network smoke; no API keys or paid model calls"]
async fn live_keyless_search_returns_relevant_english_and_chinese_sources() {
    let mut failures = Vec::new();
    for query in [
        "Rust Tokio timeout documentation",
        "杭州 西湖 官方 门票 预约",
        "site:tokio.rs tokio tutorial",
    ] {
        let (engine, hits) =
            match super::web::search_backends(&plan_backends(None, None, true, true), query, 5)
                .await
            {
                Ok(result) => result,
                Err(error) => {
                    println!("{}", json!({"query":query,"error":error.to_string()}));
                    failures.push(query);
                    continue;
                }
            };
        println!("{}", json!({"query":query,"backend":engine,"results":hits}));
        assert!(!hits.is_empty(), "{query}");
        let relevant = if query.starts_with("site:") {
            hits.iter()
                .any(|(_, _, url)| reqwest::Url::parse(url).unwrap().host_str() == Some("tokio.rs"))
        } else if query.contains("Tokio") {
            hits.iter().any(|(title, _, url)| {
                title.to_lowercase().contains("tokio")
                    && (url.contains("docs.rs/tokio/") || url.contains("tokio.rs/"))
            })
        } else {
            hits.iter().any(|(title, snippet, _)| {
                title.contains("西湖") && (title.contains("杭州") || snippet.contains("杭州"))
            })
        };
        if !relevant {
            failures.push(query);
        }
    }
    assert!(
        failures.is_empty(),
        "Queries without relevant top-five sources: {failures:?}"
    );
}

#[tokio::test]
async fn backend_failures_empty_and_invalid_results_fall_through() {
    let bad = vec![("Bad".into(), "".into(), "javascript:alert(1)".into())];
    let good = vec![("Tokio".into(), "Docs".into(), "https://tokio.rs/".into())];
    let plan = [
        Backend::Fixture(None),
        Backend::Fixture(Some(vec![])),
        Backend::Fixture(Some(bad)),
        Backend::Fixture(Some(good.clone())),
    ];
    let (_, hits) = super::web::search_backends(&plan, "tokio", 5)
        .await
        .unwrap();
    assert_eq!(hits, good);
    assert!(
        super::web::search_backends(&plan[..3], "tokio", 5)
            .await
            .is_err()
    );
}

#[test]
fn direct_duckduckgo_sources_and_unescaped_ampersands_remain_readable() {
    let hits = super::web::parse_duckduckgo(
        r#"<a class='result-link' href='https://duckduckgo.com/privacy'>Privacy & policy</a><td class='result-snippet'>A & B &#x4e2d;文</td>"#,
        5,
    );
    assert_eq!(hits[0].0, "Privacy & policy");
    assert_eq!(hits[0].1, "A & B 中文");
    assert_eq!(hits[0].2, "https://duckduckgo.com/privacy");
}

#[tokio::test]
async fn site_search_rejects_unrelated_and_deceptive_domains_before_fallback() {
    let entry = |url: &str| ("Source".into(), "Text".into(), url.into());
    let plan = [
        Backend::Fixture(Some(vec![
            entry("https://deepseek.com/"),
            entry("https://tokio.rs.evil.example/tutorial"),
            entry("https://eviltokio.rs/tutorial"),
        ])),
        Backend::Fixture(Some(vec![entry("https://tokio.rs/tokio/tutorial")])),
    ];
    let (_, hits) = super::web::search_backends(&plan, "site:tokio.rs tutorial", 5)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].2, "https://tokio.rs/tokio/tutorial");
    assert!(
        super::web::search_backends(&plan[..1], "site:tokio.rs tutorial", 5)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn scoped_search_filters_before_spending_the_result_limit() {
    let entry = |title: &str, url: &str| (title.into(), "开放时间".into(), url.into());
    let plan = [Backend::Fixture(Some(vec![
        entry("过时的转载", "https://example.net/visit"),
        entry("官方公告", "https://city.example/visit"),
    ]))];
    let (_, hits) = super::web::search_backends(&plan, "site:city.example 景区 开放时间", 1)
        .await
        .unwrap();
    assert_eq!(hits, [entry("官方公告", "https://city.example/visit")]);
}

#[test]
fn site_path_scope_requires_a_segment_boundary() {
    let query = "site:docs.example/pricing 现行价格";
    for link in [
        "https://docs.example/pricing",
        "https://www.docs.example/pricing/current",
    ] {
        assert!(super::web::matches_site(query, link), "{link}");
    }
    for link in [
        "https://docs.example/pricing-old",
        "https://docs.example/pricingevil/current",
    ] {
        assert!(!super::web::matches_site(query, link), "{link}");
    }
}

#[test]
fn tracking_variants_do_not_displace_distinct_source_revisions() {
    let hit = |title: &str, url: &str| (title.to_string(), "公告".into(), url.to_string());
    let first = "https://library.example/notices?id=145&utm_source=bulletin#hours";
    let current = "https://library.example/notices?id=146";
    let hits = super::web::clean_hits(
        vec![
            hit("图书馆公告 145", first),
            hit(
                "同一旧公告的搜索广告",
                "https://library.example/notices?utm_medium=cpc&id=145",
            ),
            hit("图书馆公告 146", current),
        ],
        2,
    );
    assert_eq!(
        hits,
        [hit("图书馆公告 145", first), hit("图书馆公告 146", current)]
    );
}

#[tokio::test]
async fn repeated_official_scope_keeps_filtering_across_fallback_providers() {
    let entry = |url: &str| ("营业时间".into(), "公告".into(), url.into());
    let official = entry("https://library.example/notices/hours");
    let plan = [
        Backend::Fixture(Some(vec![
            entry("https://library.example/notices-old/hours"),
            entry("https://directory.example/notices/hours"),
        ])),
        Backend::Fixture(Some(vec![official.clone()])),
    ];
    let (_, hits) = super::web::search_backends(
        &plan,
        "site:library.example/notices 营业时间 SITE:Library.Example/notices/",
        1,
    )
    .await
    .unwrap();
    assert_eq!(hits, [official]);
}
