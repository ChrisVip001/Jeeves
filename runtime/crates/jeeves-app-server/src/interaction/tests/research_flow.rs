use super::*;
use jeeves_provider::mock::Mock;
use jeeves_state::TaskStatus;
use jeeves_tools::web::{fetch_url_with_primary, search_web_with_primary};

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
    tools.register(fetch_url_with_primary(Some(handler)));
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
    let queue = jeeves_state::dispatch::TaskDispatch::new(server.pool.clone());
    tokio::time::timeout(Duration::from_secs(10), async {
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
