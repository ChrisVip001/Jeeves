//! Regression from IKEA China's public product and store pages. The live test uses
//! the actual web tool with a scripted model so no model service or account is required.

use super::*;

const PRODUCT: &str = "https://www.ikea.cn/cn/zh/p/20586788/?artType=art";
const STORE: &str = "https://www.ikea.cn/cn/zh/stores/shenzhen/";
const INVENTED: &str = "已确认该型号在深圳商场的到手价为 179 元。";
const PAGES: [(&str, &str, &str); 2] = [
    ("merchant-product", PRODUCT, "205.867.88"),
    ("merchant-store", STORE, "深圳商场"),
];

fn request(mode: &str, date: &str) -> String {
    let query = json!({
        "host":"www.ikea.cn",
        "store":"深圳商场",
        "sku":"205.867.88",
        "variant":"白色，35x45x77 厘米",
        "date":date,
        "conditions":"无优惠",
        "url":PRODUCT,
    });
    format!("{mode}\nprice {query}")
}

fn replies() -> Vec<CreateResponse> {
    let mut replies: Vec<_> = PAGES.iter().map(|(id, url, _)| fetch(id, url)).collect();
    replies.push(Mock::text(INVENTED));
    replies
}

fn assert_unresolved(reply: &str) {
    assert!(reply.contains("待核实"), "{reply}");
    for unsupported in [INVENTED, "CNY 179", "固定到手总价为"] {
        assert!(!reply.contains(unsupported), "{reply}");
    }
}

fn assert_read_links(reply: &str) {
    assert_unresolved(reply);
    assert!(reply.contains("缺少可核对的结构化门店报价记录"), "{reply}");
    for source in [PRODUCT, STORE] {
        assert!(reply.contains(source), "{reply}");
    }
}

async fn quick(server: &Server, date: &str) -> (String, usize) {
    let events = send(
        server,
        &request("/web_research", date),
        "merchant-catalog-quick",
        None,
    )
    .await
    .unwrap();
    let mut observations = 0;
    for (id, _, marker) in PAGES {
        let observed = events
            .iter()
            .find_map(|event| match event {
                ServerNotification::ToolCompleted(call) if call.tool_use_id == id => Some(call),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing {id} tool outcome"));
        if observed.status == ToolStatus::Ok && observed.output.contains(marker) {
            observations += 1;
        } else {
            let diagnostic = observed.output.lines().next().unwrap_or_default();
            eprintln!("Quick read {id} did not observe product or store: {diagnostic}");
        }
    }
    let reply = events
        .into_iter()
        .find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply),
            _ => None,
        })
        .unwrap();
    assert_unresolved(&reply);
    (reply, observations)
}

async fn deep(server: &Server, date: &str, timeout: Duration) -> (String, usize) {
    send(
        server,
        &request("/deep_research", date),
        "merchant-catalog-deep",
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
    settled_with_timeout(server, id, TaskStatus::NeedsAttention, timeout).await;
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
    let state: serde_json::Value = serde_json::from_str(&saved).unwrap();
    let pages = state["observations"].as_array().unwrap();
    let mut observations = 0;
    for (id, url, marker) in PAGES {
        if let Some(page) = pages
            .iter()
            .find(|page| page["receipt"]["Page"]["requested"] == url)
        {
            assert_eq!(page["receipt"]["Page"]["effective"], url);
            assert_eq!(page["receipt"]["Page"]["complete"], true);
            if page["text"].as_str().unwrap().contains(marker) {
                observations += 1;
                continue;
            }
        }
        eprintln!("Deep read {id} did not persist a recognizable page body.");
    }
    let session = crate::research::Session::open(server, Some(id), "Continue")
        .await
        .unwrap()
        .unwrap();
    let (reply, complete) = session.render().await;
    assert!(!complete);
    assert_unresolved(&reply);
    (reply, observations)
}

fn replay(server: &mut Server) {
    install_store_feeds(
        server,
        replies(),
        vec![
            (
                PRODUCT,
                "RÅSKOG 拉斯克\n货号205.867.88\n手推车，白色，35x45x77 厘米\n¥179.00".into(),
            ),
            (STORE, "深圳商场\n商场活动仅在该实体门店适用。".into()),
        ],
    );
}

#[tokio::test]
async fn merchant_catalog_price_and_store_activity_do_not_bind_to_checkout() {
    let (_home, mut server, _) = setup().await;
    let date = "2026-11-27";
    replay(&mut server);
    let (reply, observed) = quick(&server, date).await;
    assert_eq!(observed, PAGES.len());
    assert_read_links(&reply);
    replay(&mut server);
    let (reply, observed) = deep(&server, date, Duration::from_secs(10)).await;
    assert_eq!(observed, PAGES.len());
    assert_read_links(&reply);
}

#[tokio::test]
#[ignore = "Opt-in, performs public reads of IKEA China product and store pages"]
async fn live_merchant_catalog_stays_unresolved_in_both_research_modes() {
    let day = chrono::Utc::now()
        .with_timezone(&chrono_tz::Asia::Shanghai)
        .date_naive()
        .to_string();
    let (_home, mut server, _) = setup().await;
    install(&mut server, replies());
    let mut tools = server.tools.without("fetch_url");
    tools.register(fetch_url());
    server.tools = Arc::new(tools);
    let (quick_reply, quick_observed) = quick(&server, &day).await;

    let real_tools = server.tools.clone();
    install(&mut server, replies());
    server.tools = real_tools;
    let (deep_reply, deep_observed) = deep(&server, &day, Duration::from_secs(55)).await;
    assert_eq!(
        (quick_observed, deep_observed),
        (PAGES.len(), PAGES.len()),
        "Both modes stayed unresolved; live acceptance requires both recognizable public pages in each mode"
    );
    assert_read_links(&quick_reply);
    assert_read_links(&deep_reply);
}
