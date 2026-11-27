use super::*;
use jeeves_provider::mock::Mock;
use jeeves_state::TaskStatus;
use jeeves_tools::web::{WebReceipt, fetch_url, search_web_with_primary};

const RESERVOIR: &str = "https://93.184.216.34/fixture/reservoir";
const TRANSIT: &str = "https://93.184.216.34/fixture/transit";
const WATER: &str = "On May 4, the reservoir stood at 68 percent of capacity.";
const TRAINS: &str = "On May 4, the northbound trains stopped for scheduled track works.";
const QUESTIONS: &str =
    "How full was the reservoir on May 4?\nWhat happened to northbound trains on May 4?";
const MODEL_ONLY: &str = "Unverified model prose must not be delivered.";

fn install(server: &mut Server, responses: Vec<CreateResponse>) -> Arc<Mutex<Vec<String>>> {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let handler: jeeves_tools::Handler = Arc::new(move |_, args| {
        let log = log.clone();
        let (action, text) = if let Some(url) = args["url"].as_str() {
            (
                url.to_owned(),
                match url {
                    RESERVOIR => WATER.to_string(),
                    TRANSIT => TRAINS.to_string(),
                    _ => panic!("unexpected fixture URL"),
                },
            )
        } else {
            (
                "search".into(),
                format!("URL: {RESERVOIR}\n{WATER}\nURL: {TRANSIT}\n{TRAINS}"),
            )
        };
        Box::pin(async move {
            log.lock().unwrap().push(action);
            Ok(text)
        })
    });
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(handler.clone())));
    tools.register(bound_fetch_fixture(handler));
    server.tools = Arc::new(tools);
    server.install_provider(Resolved {
        client: Arc::new(Mock::streaming(responses)),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32_000,
    });
    calls
}

/// The test corpus is the direct HTTP response. Keep DNS and external I/O out of replay.
pub(super) fn bound_fetch_fixture(handler: jeeves_tools::Handler) -> jeeves_tools::Tool {
    let mut tool = fetch_url();
    tool.handler = Arc::new(move |ctx, args| {
        let handler = handler.clone();
        Box::pin(async move {
            let url = jeeves_tools::require_str(&args, "url")?;
            let body = handler(ctx.clone(), args).await?;
            if let Some(capture) = ctx.web_receipt {
                capture.set(WebReceipt::Page {
                    requested: url.clone(),
                    effective: Some(url.clone()),
                    complete: true,
                });
            }
            if let Some(cwd) = ctx.task_run_id.and(ctx.cwd) {
                jeeves_tools::snapshot::save(&cwd, &url, &body)?;
            }
            Ok(body)
        })
    });
    tool
}

fn fetch(id: &str, url: &str) -> CreateResponse {
    Mock::tool_use(id, "fetch_url", json!({"url":url}))
}

fn cite(id: &str, question: u32, source: u32, quote: &str) -> CreateResponse {
    Mock::tool_use(
        id,
        "research_evidence",
        json!({
            "action":"record","question":question,"citations":[{"source":source,"quote":quote}]
        }),
    )
}

#[tokio::test]
async fn quick_research_preserves_context_and_reports_a_missing_required_source() {
    let (_home, mut server, _) = setup().await;
    let calls = install(
        &mut server,
        vec![
            fetch("open", RESERVOIR),
            Mock::tool_use(
                "context",
                "research_evidence",
                json!({
                    "action":"context", "question":1,
                    "citations":[{"source":1,"quote":WATER}],
                    "gap":"No reading for May 5 was established."
                }),
            ),
            Mock::text(MODEL_ONLY),
        ],
    );
    let events = send(
        &server,
        &format!(
            "/web_research\nOpen {RESERVOIR} and <{TRANSIT}>. How full was the reservoir on May 5?"
        ),
        "quick-research-required-sources",
        None,
    )
    .await
    .unwrap();
    let reply = events
        .iter()
        .find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(
        reply.contains("Unresolved:") && reply.contains(WATER),
        "{reply}"
    );
    assert!(
        reply.contains("No reading for May 5 was established."),
        "{reply}"
    );
    assert!(
        reply.contains(&format!("Required source unread: <{TRANSIT}>")),
        "{reply}"
    );
    assert_eq!(*calls.lock().unwrap(), [RESERVOIR]);
}

#[tokio::test]
async fn quick_reports_observed_excerpts_and_keeps_unopened_and_invented_claims_out() {
    let (_home, mut server, _) = setup().await;
    let calls = install(
        &mut server,
        vec![
            Mock::tool_use(
                "search",
                "search_web",
                json!({"query":"water level and train closure May 4"}),
            ),
            cite("unopened", 2, 1, TRAINS),
            fetch("open", RESERVOIR),
            cite(
                "fabrication",
                1,
                1,
                "On May 4, the reservoir stood at 88 percent of capacity.",
            ),
            cite("valid", 1, 1, WATER),
            Mock::text(MODEL_ONLY),
        ],
    );
    let events = send(
        &server,
        &format!("/web_research\n{QUESTIONS}"),
        "quick-research",
        None,
    )
    .await
    .unwrap();
    let reply = events
        .iter()
        .find_map(|e| match e {
            ServerNotification::TurnCompleted(done) => Some(&done.reply),
            _ => None,
        })
        .unwrap();
    assert!(reply.contains(WATER) && reply.contains(RESERVOIR) && reply.contains("Unresolved:"));
    assert!(
        !reply.contains(TRAINS) && !reply.contains("88 percent") && !reply.contains(MODEL_ONLY),
        "{reply}"
    );
    assert!(events.iter().all(
        |e| !matches!(e, ServerNotification::TextDelta(delta) if delta.delta.contains(MODEL_ONLY))
    ));
    assert_eq!(*calls.lock().unwrap(), ["search", RESERVOIR]);
    assert!(server.tasks.list(10).await.unwrap().is_empty());
}

async fn settled(server: &Server, id: &str, wanted: TaskStatus) {
    settled_with_timeout(server, id, wanted, Duration::from_secs(10)).await;
}

async fn settled_with_timeout(server: &Server, id: &str, wanted: TaskStatus, timeout: Duration) {
    let queue = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    tokio::time::timeout(timeout, async {
        loop {
            let row = server.tasks.get(id).await.unwrap().unwrap();
            if row.status == wanted
                && queue
                    .get(id)
                    .await
                    .unwrap()
                    .is_some_and(|r| r.state == "finished")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("research task did not settle as {wanted}"));
}

async fn partial_deep_task() -> (tempfile::TempDir, String) {
    let (home, mut server, _) = setup().await;
    let calls = install(
        &mut server,
        vec![
            fetch("water", RESERVOIR),
            cite("fill", 1, 1, WATER),
            Mock::text(MODEL_ONLY),
        ],
    );
    send(
        &server,
        &format!("/deep_research\n{QUESTIONS}"),
        "deep-research",
        None,
    )
    .await
    .unwrap();
    let rows = server
        .tasks
        .list_for_session(MAIN_SESSION, 10)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    let id = rows[0].id.clone();
    settled(&server, &id, TaskStatus::NeedsAttention).await;
    assert!(
        server
            .completions
            .latest_delivery(&id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(*calls.lock().unwrap(), [RESERVOIR]);
    (home, id)
}

#[tokio::test]
async fn deep_research_reuses_observations_after_restart_and_publishes_only_the_rendered_result() {
    let (home, id) = std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(partial_deep_task())
    })
    .join()
    .unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let mut calls = None;
    let server = Server::boot_fixture(
        pool,
        jeeves_config::Settings {
            home: home.path().to_path_buf(),
            ..Default::default()
        },
        |server| {
            calls = Some(install(
                server,
                vec![
                    Mock::tool_use("scope", "research_evidence", json!({"action":"status"})),
                    fetch("same-url", RESERVOIR),
                    fetch("trains", TRANSIT),
                    cite("northbound", 2, 2, TRAINS),
                    Mock::text(MODEL_ONLY),
                ],
            ));
        },
    )
    .await;
    send(
        &server,
        "Continue the missing research question.",
        "continue-deep",
        Some(id.clone()),
    )
    .await
    .unwrap();
    settled(&server, &id, TaskStatus::Done).await;
    let receipt = server
        .completions
        .latest_delivery(&id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.status, "done");
    let text = receipt.summary.unwrap();
    assert!(text.contains(WATER) && text.contains(TRAINS));
    let verification = jeeves_state::verification::Verifications::new(server.pool.clone())
        .latest(&id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        Some(verification.run_id.as_str()),
        receipt.run_id.as_deref()
    );
    let hash = jeeves_state::ledger::digest_of(text.as_bytes());
    assert!(verification.checks_json.contains(&hash));
    assert!(!text.contains(MODEL_ONLY) && !text.contains("Unresolved:"));
    let saved: String =
        sqlx::query_scalar("SELECT state_json FROM research_evidence WHERE task_id=?")
            .bind(&id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let saved: serde_json::Value = serde_json::from_str(&saved).unwrap();
    for (i, (url, quote)) in [(RESERVOIR, WATER), (TRANSIT, TRAINS)].iter().enumerate() {
        let observation = &saved["observations"][i];
        assert_eq!(observation["receipt"]["Page"]["requested"], *url);
        assert_eq!(observation["receipt"]["Page"]["effective"], *url);
        assert_eq!(observation["receipt"]["Page"]["complete"], true);
        assert_eq!(
            observation["digest"],
            jeeves_state::ledger::digest_of(quote.as_bytes())
        );
    }
    assert_eq!(*calls.unwrap().lock().unwrap(), [TRANSIT]);
}

#[tokio::test]
async fn a_provider_failure_still_returns_observed_partial_evidence() {
    let (_home, mut server, _) = setup().await;
    let calls = install(&mut server, vec![]);
    server.install_provider(Resolved {
        client: Arc::new(Mock::with_outcomes(vec![
            Ok(fetch("read-one", RESERVOIR)),
            Ok(cite("cite-one", 1, 1, WATER)),
            Err(ProviderError::Api(
                "simulated private provider error".into(),
            )),
        ])),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32_000,
    });
    let events = send(
        &server,
        &format!("/web_research\n{QUESTIONS}"),
        "provider-stops",
        None,
    )
    .await
    .unwrap();
    let reply = events
        .iter()
        .find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(&done.reply),
            _ => None,
        })
        .unwrap();
    assert!(
        reply.contains(WATER) && reply.contains("Unresolved:") && reply.contains("stopped early")
    );
    assert!(!reply.contains("private provider error") && !reply.contains(TRAINS));
    assert_eq!(*calls.lock().unwrap(), [RESERVOIR]);
}

#[tokio::test]
async fn quick_research_cites_only_bound_complete_fetches_and_uses_the_final_url() {
    const BEFORE: &str = "https://93.184.216.34/old-report?date=4";
    const FINAL: &str = "https://93.184.216.34/final-report?date=4";
    const UNBOUND: &str = "https://93.184.216.34/unbound";
    const PARTIAL: &str = "https://93.184.216.34/partial";
    let (_home, mut server, _) = setup().await;
    install(
        &mut server,
        vec![
            fetch("unbound", UNBOUND),
            fetch("partial", PARTIAL),
            cite("attempt-to-bind", 1, 1, WATER),
            fetch("redirected", BEFORE),
            cite("valid", 1, 1, WATER),
            Mock::text(MODEL_ONLY),
        ],
    );
    let mut tools = server.tools.without("fetch_url");
    let mut tool = fetch_url();
    tool.handler = Arc::new(|ctx, args| {
        Box::pin(async move {
            let url = jeeves_tools::require_str(&args, "url")?;
            let (effective, complete) = match url.as_str() {
                UNBOUND => (None, true),
                PARTIAL => (Some(PARTIAL), false),
                BEFORE => (Some(FINAL), true),
                _ => panic!("unexpected fixture URL"),
            };
            ctx.web_receipt.unwrap().set(WebReceipt::Page {
                requested: url,
                effective: effective.map(str::to_owned),
                complete,
            });
            Ok(WATER.into())
        })
    });
    tools.register(tool);
    server.tools = Arc::new(tools);

    let events = send(
        &server,
        "/web_research\nHow full was the reservoir on May 4?",
        "quick-bound-read",
        None,
    )
    .await
    .unwrap();
    for id in ["unbound", "partial", "attempt-to-bind"] {
        assert!(
            events.iter().any(|event| matches!(event,
                ServerNotification::ToolCompleted(call)
                    if call.tool_use_id == id && call.status == ToolStatus::Error
            )),
            "{id}: {events:?}"
        );
    }
    let reply = events
        .iter()
        .find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(&done.reply),
            _ => None,
        })
        .unwrap();
    assert!(reply.contains(WATER) && reply.contains(FINAL), "{reply}");
    for invalid in [BEFORE, UNBOUND, PARTIAL, MODEL_ONLY] {
        assert!(!reply.contains(invalid), "{reply}");
    }
}

const STORE_FEED: &str = "https://shop.example/quotes";
const STORE_FEED_UPDATED: &str = "https://shop.example/updated";
const STORE_QUERY: &str = r#"price {"host":"shop.example","store":"南山店","sku":"A250","variant":"250ml","date":"2026-11-27","conditions":"无优惠","url":"https://shop.example/quotes"}"#;

fn store_offer(amount: &str) -> serde_json::Value {
    json!({
        "store":"南山店","sku":"A250","variant":"250ml","date":"2026-11-27",
        "conditions":"无优惠","currency":"CNY","priceKind":"checkout_total","amount":amount
    })
}

fn install_store_feeds(
    server: &mut Server,
    responses: Vec<CreateResponse>,
    feeds: Vec<(&'static str, String)>,
) -> Arc<Mutex<Vec<String>>> {
    let calls = install(server, responses);
    let recorded = calls.clone();
    let handler: jeeves_tools::Handler = Arc::new(move |_, args| {
        let url = args["url"].as_str().unwrap().to_owned();
        let body = feeds.iter().find(|(key, _)| *key == url).unwrap().1.clone();
        let recorded = recorded.clone();
        Box::pin(async move {
            recorded.lock().unwrap().push(url);
            Ok(body)
        })
    });
    let mut tools = server.tools.without("fetch_url");
    tools.register(bound_fetch_fixture(handler));
    server.tools = Arc::new(tools);
    calls
}

#[tokio::test]
async fn quick_research_prices_come_from_the_scoped_record_and_the_host_renders_conflicts() {
    for conflict in [false, true] {
        let (_home, mut server, _) = setup().await;
        let mut wrong_store = store_offer("199");
        wrong_store["store"] = json!("福田店");
        let mut wrong_date = store_offer("209");
        wrong_date["date"] = json!("2026-11-26");
        let mut starting = store_offer("219");
        starting["priceKind"] = json!("starting_at");
        let mut other_terms = store_offer("179");
        other_terms["conditions"] = json!("仅新客");
        let mut responses = vec![
            fetch("store-feed", STORE_FEED),
            cite("model-cannot-bind-price", 1, 1, "199"),
        ];
        if conflict {
            responses.push(fetch("conflict", STORE_FEED_UPDATED));
        }
        responses.push(Mock::text("最终价格为 999 元。"));
        install_store_feeds(
            &mut server,
            responses,
            vec![
                (STORE_FEED, json!({"offers":[wrong_store,wrong_date,starting,other_terms,store_offer("299")]}).to_string()),
                (STORE_FEED_UPDATED, json!({"offers":[store_offer("309")]}).to_string()),
            ],
        );
        let events = send(
            &server,
            &format!("/web_research\n{STORE_QUERY}"),
            "price",
            None,
        )
        .await
        .unwrap();
        let reply = events
            .iter()
            .find_map(|event| match event {
                ServerNotification::TurnCompleted(done) => Some(&done.reply),
                _ => None,
            })
            .unwrap();
        assert!(
            reply.contains("CNY 299.00") && reply.contains(STORE_FEED),
            "{reply}"
        );
        assert_eq!(reply.contains("同一口径出现不同报价"), conflict, "{reply}");
        assert_eq!(reply.contains("CNY 309.00"), conflict, "{reply}");
        assert_eq!(reply.contains("待核实"), conflict, "{reply}");
        for absent in ["CNY 199", "CNY 209", "CNY 219", "CNY 179", "999 元"] {
            assert!(!reply.contains(absent), "{reply}");
        }
        assert!(events.iter().any(|event| matches!(event,
            ServerNotification::ToolCompleted(call) if call.tool_use_id == "model-cannot-bind-price"
                && call.status == ToolStatus::Error
        )));
    }
}

#[tokio::test]
async fn deep_research_does_not_deliver_a_conflicted_price() {
    let (_home, mut server, _) = setup().await;
    install_store_feeds(
        &mut server,
        vec![
            fetch("store-feed", STORE_FEED),
            fetch("second-price", STORE_FEED_UPDATED),
            Mock::text("价格已经核实。"),
        ],
        vec![
            (
                STORE_FEED,
                json!({"offers":[store_offer("299")]}).to_string(),
            ),
            (
                STORE_FEED_UPDATED,
                json!({"offers":[store_offer("309")]}).to_string(),
            ),
        ],
    );
    send(
        &server,
        &format!("/deep_research\n{STORE_QUERY}"),
        "deep-price",
        None,
    )
    .await
    .unwrap();
    let jobs = server
        .tasks
        .list_for_session(MAIN_SESSION, 10)
        .await
        .unwrap();
    assert_eq!(jobs.len(), 1);
    let id = &jobs[0].id;
    settled(&server, id, TaskStatus::NeedsAttention).await;
    assert!(
        server
            .completions
            .latest_delivery(id)
            .await
            .unwrap()
            .is_none()
    );
    let saved: String =
        sqlx::query_scalar("SELECT state_json FROM research_evidence WHERE task_id=?")
            .bind(id)
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let data: serde_json::Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(data["priceQueries"]["0"]["store"], "南山店");
    assert_eq!(data["observations"].as_array().unwrap().len(), 2);
}

#[path = "research_flow/merchant.rs"]
mod merchant;
