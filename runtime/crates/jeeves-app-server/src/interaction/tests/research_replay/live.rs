//! Opt-in live model with fixture-only web calls and a bounded request budget.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

const MODEL: &str = "deepseek-flash";
const WEB_CAP: usize = 4;
const REQUEST_CAP: usize = 8;
const MAX_OUTPUT: i32 = 1_024;
const MAX_INPUT_ESTIMATE: usize = 12_000;

pub(in crate::interaction::tests) struct Bounded {
    inner: Arc<dyn Provider>,
    sent: AtomicUsize,
    usage: Mutex<Vec<jeeves_provider::Usage>>,
}

impl Provider for Bounded {
    fn create(&self, request: CreateRequest) -> Reply<'_> {
        self.run(request, None)
    }

    fn stream(&self, request: CreateRequest, sink: TextSink) -> Reply<'_> {
        self.run(request, Some(sink))
    }
}

impl Bounded {
    fn run(&self, mut request: CreateRequest, sink: Option<TextSink>) -> Reply<'_> {
        Box::pin(async move {
            let tokens = jeeves_provider::tokens::estimate_messages(&request.messages)
                + jeeves_provider::tokens::estimate_tools(&request.tools)
                + jeeves_provider::tokens::estimate_text(
                    &request.system_text().unwrap_or_default(),
                );
            if request.model != MODEL
                || tokens > MAX_INPUT_ESTIMATE
                || self.sent.fetch_add(1, Ordering::Relaxed) >= REQUEST_CAP
            {
                return Err(ProviderError::Api("Fixture model cap reached".into()));
            }
            request.max_tokens = request.max_tokens.min(MAX_OUTPUT);
            let result = match sink {
                Some(sink) => self.inner.stream(request, sink).await,
                None => self.inner.create(request).await,
            };
            // The provider can include credential fragments in error text. Log only the status.
            let response = result.map_err(|err| match err {
                ProviderError::Http { status, .. } => {
                    ProviderError::Api(format!("Fixture DeepSeek HTTP {status}"))
                }
                ProviderError::Network(_) => {
                    ProviderError::Api("Fixture DeepSeek network error".into())
                }
                other => ProviderError::Api(format!(
                    "Fixture DeepSeek response error: {}",
                    match other {
                        ProviderError::Parse(_) => "decode",
                        ProviderError::Incomplete(_) => "stream incomplete",
                        ProviderError::ContextOverflow { .. } => "context limit",
                        _ => "API",
                    }
                )),
            })?;
            self.usage.lock().unwrap().push(response.usage);
            Ok(response)
        })
    }

    pub(in crate::interaction::tests) fn accounting(&self) -> Value {
        let usage = self.usage.lock().unwrap();
        json!({
            "requests":self.sent.load(Ordering::Relaxed).min(REQUEST_CAP),
            "responses":usage.len(),
            "inputTokens":usage.iter().map(|u| u.input_tokens).sum::<i64>(),
            "outputTokens":usage.iter().map(|u| u.output_tokens).sum::<i64>(),
        })
    }
}

fn live_credential() -> Result<(String, String), &'static str> {
    use jeeves_config::models::{SecretStore, SecretVault, read_models};

    const OFFICIAL: &str = "https://api.deepseek.com";
    let configured_home = std::env::var_os("JEEVES_TEST_MODEL_HOME");
    let configured_profile = std::env::var("JEEVES_TEST_MODEL_PROFILE").ok();
    let (Some(path), Some(profile_id)) = (configured_home.as_ref(), configured_profile.as_ref())
    else {
        if configured_home.is_some() || configured_profile.is_some() {
            return Err("Set both JEEVES_TEST_MODEL_HOME and JEEVES_TEST_MODEL_PROFILE");
        }
        return std::env::var("DEEPSEEK_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .map(|key| (key, OFFICIAL.into()))
            .ok_or("No configured DeepSeek credential is accessible; 0 API requests");
    };
    let path = std::path::Path::new(path);
    let models =
        read_models(path).map_err(|_| "Cannot read the configured Jeeves model profiles")?;
    let profile = models
        .profiles
        .iter()
        .find(|profile| {
            profile.id == *profile_id
                && profile.provider == "deepseek"
                && profile.model.as_deref() == Some(MODEL)
        })
        .ok_or("No matching configured DeepSeek Flash profile")?;
    let base = profile
        .base_url
        .as_deref()
        .unwrap_or(OFFICIAL)
        .trim_end_matches('/');
    if !matches!(base, OFFICIAL | "https://api.deepseek.com/v1") {
        return Err("The configured profile must use the official DeepSeek endpoint");
    }
    let key = match profile.key_source.as_deref() {
        Some("keychain" | "file") => SecretStore::auto()
            .versioned_credential(path, &profile.credential_slot.account(profile_id))
            .map(|(key, _)| key),
        Some(source) => source
            .strip_prefix("env:")
            .and_then(|name| std::env::var(name).ok()),
        None => None,
    };
    key.filter(|key| !key.trim().is_empty())
        .map(|key| (key, base.to_owned()))
        .ok_or("The configured DeepSeek credential is unavailable; 0 API requests")
}

pub(in crate::interaction::tests) fn live_provider() -> (Resolved, Arc<Bounded>) {
    let (key, base_url) = live_credential().unwrap_or_else(|error| panic!("{error}"));
    let settings = jeeves_config::Settings {
        provider: "deepseek".into(),
        model: Some(MODEL.into()),
        small_model: Some(MODEL.into()),
        base_url: Some(base_url),
        api_key: Some(key),
        llm_retries: 0,
        llm_timeout_secs: 40,
        ..Default::default()
    };
    let catalog = jeeves_provider::catalog::Catalog::load(&settings.home);
    let mut resolved = jeeves_provider::resolve_screen(&settings, &catalog)
        .unwrap_or_else(|_| panic!("DeepSeek fixture profile is unavailable"));
    let bounded = Arc::new(Bounded {
        inner: resolved.client.clone(),
        sent: AtomicUsize::new(0),
        usage: Mutex::new(Vec::new()),
    });
    resolved.client = bounded.clone();
    (resolved, bounded)
}

type Trace = Arc<Mutex<Vec<Value>>>;

fn tools(server: &mut Server, pool: &[Record], deep: bool) -> Trace {
    let mut registry = (*server.tools).clone();
    for name in server.tools.names() {
        if !matches!(name, "search_web" | "fetch_url") && !(deep && name == "write_file") {
            registry = registry.without(name);
        }
    }
    let trace: Trace = Arc::new(Mutex::new(Vec::new()));
    let cap = Arc::new(AtomicUsize::new(0));
    let titles = |r: Record| match r {
        Traffic => "Port recovery update: vessel arrivals",
        Mirror => "Republished port recovery update: vessel arrivals",
        Access => "USACE channel restoration bulletin",
        Throughput => "Maryland Port Commission: terminal container statistics",
    };
    let ordered_results = pool
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "Source {}: {}\nURL: {}\n{}",
                i + 1,
                titles(*r),
                r.url(),
                match r {
                    Traffic | Mirror =>
                        "Port activity shows signs of recovery; vessel traffic analysis.",
                    Access => "Federal navigation access after the bridge collapse.",
                    Throughput => "Container TEU statistics for terminal operations.",
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    // The replay corpus supplies bound bodies and writes task snapshots without external reads.
    let search = search_web_with_primary(Some(Arc::new(move |_, _| {
        let result = ordered_results.clone();
        Box::pin(async move { Ok(result) })
    })));
    let corpus = pool.iter().map(|r| (r.url(), r.text())).collect::<Vec<_>>();
    let pages = corpus.clone();
    let fetch = super::super::research_flow::bound_fetch_fixture(Arc::new(move |_, args| {
        let body = pages
            .iter()
            .find(|(url, _)| args["url"].as_str() == Some(url.as_str()))
            .expect("only fixture URLs can enter this adapter")
            .1
            .to_owned();
        Box::pin(async move { Ok(body) })
    }));
    for (mut tool, kind) in [(search, "search"), (fetch, "fetch")] {
        let actual = tool.handler;
        let trace = trace.clone();
        let cap = cap.clone();
        let corpus = corpus.clone();
        tool.handler = Arc::new(move |ctx, args| {
            let actual = actual.clone();
            let trace = trace.clone();
            let corpus = corpus.clone();
            let cap = cap.clone();
            Box::pin(async move {
                let url = args["url"].as_str().unwrap_or_default();
                if cap.fetch_add(1, Ordering::Relaxed) >= WEB_CAP {
                    trace
                        .lock()
                        .unwrap()
                        .push(json!({"kind":kind,"result":"budget"}));
                    anyhow::bail!("Fixture web budget exhausted; report any unverified items.");
                }
                if kind == "fetch" && !corpus.iter().any(|(allowed, _)| allowed == url) {
                    trace
                        .lock()
                        .unwrap()
                        .push(json!({"kind":kind,"result":"unavailable"}));
                    anyhow::bail!("That URL is not available in the fixture.");
                }
                let observed = if kind == "search" {
                    json!({"kind":kind})
                } else {
                    let record = [Traffic, Mirror, Access, Throughput]
                        .into_iter()
                        .find(|r| r.url() == url)
                        .unwrap();
                    json!({"kind":kind,"record":format!("{record:?}"),"family":record.family()})
                };
                let result = actual(ctx, args).await;
                trace.lock().unwrap().push(json!({
                    "call":observed,"ok":result.is_ok(),
                }));
                result
            })
        });
        registry = registry.without(&tool.name);
        registry.register_with_effect(tool, jeeves_tools::ToolEffect::Read);
    }
    if deep {
        let mut writer = registry.get("write_file").unwrap().clone();
        let mutator = writer.handler;
        writer.handler = Arc::new(move |ctx, args| {
            let mutator = mutator.clone();
            Box::pin(async move {
                if !matches!(args["path"].as_str(), Some("claims.json" | "report.md"))
                    || args["content"].as_str().is_none_or(|s| s.len() > 12_000)
                {
                    anyhow::bail!("Only bounded research artifacts are allowed in the fixture.");
                }
                mutator(ctx, args).await
            })
        });
        registry = registry.without("write_file");
        registry.register_exclusive(writer);
    }
    server.tools = Arc::new(registry);
    trace
}

#[tokio::test]
#[ignore = "opt-in: DeepSeek API cost; search and fetch are offline fixtures"]
async fn deepseek_research_replay() {
    for (arm, ranking) in [
        ("traffic-first", vec![Traffic, Mirror, Access, Throughput]),
        ("teu-first", vec![Throughput, Mirror, Access, Traffic]),
        ("no-teu", vec![Traffic, Mirror, Access]),
    ] {
        let (_home, mut server, _) = setup().await;
        let trace = tools(&mut server, &ranking, false);
        let (resolved, provider) = live_provider();
        server.install_provider(resolved);
        let result = send(&server, ASK, arm, None).await;
        let reply = result.as_ref().ok().and_then(|events| {
            events.iter().find_map(|event| match event {
                ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
                _ => None,
            })
        });
        let trace = trace.lock().unwrap().clone();
        println!(
            "RESEARCH_REPLAY {}",
            json!({"arm":arm,"reply":reply,"trace":trace,"accounting":provider.accounting()})
        );
        assert!(reply.is_some(), "No completed reply in {arm}");
    }

    let ranking = [Traffic, Mirror, Access, Throughput];
    let (_home, workspace, turn) = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(pause_after_first_read(&ranking))
    })
    .join()
    .unwrap();
    let pool = jeeves_state::open(&workspace.join("state.db"))
        .await
        .unwrap();
    let (resolved, provider) = live_provider();
    let mut trace = None;
    let server = Server::boot_fixture(
        pool,
        jeeves_config::Settings {
            home: workspace.clone(),
            write_enabled: true,
            ..Default::default()
        },
        |server| {
            trace = Some(tools(server, &ranking, true));
            server.install_provider(resolved);
        },
    )
    .await;
    assert!(!workspace.join("report.md").exists());
    let cp = server.checkpoints.get(&turn).await.unwrap().unwrap();
    assert!(cp.messages_json.contains(TRAFFIC));
    rpc(&server, "task/signal", json!({
        "conditionId":format!("{TASK}:input"),"source":"replay-fixture","eventKey":"continue","eventId":"one",
        "data":{"instruction":"Continue the original investigation. The draft is not final. Use the available evidence; write claims.json (array of claimId, text, sourceRefs and verbatim quote) and report.md with citations when ready."}
    })).await.unwrap();
    let row = tokio::time::timeout(Duration::from_secs(140), async {
        loop {
            let row = server.tasks.get(TASK).await.unwrap().unwrap();
            if row.status.is_terminal() {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("live replay timed out");
    let trace = trace.unwrap().lock().unwrap().clone();
    let report = std::fs::read_to_string(workspace.join("report.md")).ok();
    let checks = serde_json::from_str::<Value>(&row.checks_json).unwrap_or(Value::Null);
    let claims_present = workspace.join("claims.json").is_file();
    println!(
        "RESEARCH_REPLAY {}",
        json!({
            "arm":"restart","reply":report,"trace":trace,"status":format!("{:?}",row.status),
            "lastStopReason":row.last_stop_reason,"checks":checks,"claimsPresent":claims_present,
            "accounting":provider.accounting(),
        })
    );
}

#[tokio::test]
#[ignore = "opt-in: real model with a four-call fixed web fixture"]
async fn research_evidence_live_probe() {
    let (_home, mut server, _) = setup().await;
    let trace = tools(&mut server, &[Traffic, Mirror, Access, Throughput], false);
    let (resolved, provider) = live_provider();
    server.install_provider(resolved);
    let ask = "/web_research\n巴尔的摩港大桥坍塌后航道恢复了吗？\n2024 年 7–8 月进港船只与 2023 年同期相比如何？\n2024 年 7–8 月港务局码头集装箱吞吐量与 2023 年同期相比如何？";
    let result = send(&server, ask, "evidence-v1-live", None).await;
    let reply = result.as_ref().ok().and_then(|events| {
        events.iter().find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
            _ => None,
        })
    });
    let trace = trace.lock().unwrap().clone();
    println!(
        "EVIDENCE_V1_LIVE {}",
        json!({"reply":reply,"trace":trace,"accounting":provider.accounting()})
    );
    assert!(reply.is_some());
}
