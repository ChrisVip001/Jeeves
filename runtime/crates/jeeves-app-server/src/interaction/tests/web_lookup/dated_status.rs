use super::*;

const ASK: &str = "我订了 2030-11-08 早晨 6 点半从甲城到乙城的飞机（经济舱），应该订哪个座位看窗外的风景才不被遮挡视线？";
const OLD: &str =
    "https://www.flightstats.com/v2/flight-tracker/QZ/123?date=7&flightId=9123&month=11&year=2030";
const TARGET: &str =
    "https://www.flightstats.com/v2/flight-tracker/QZ/123?date=8&flightId=8123&month=11&year=2030";
const FORGED: &str = "2030-11-07 QZ777 实际 06:30，2030-11-08 QZ123 已确认，建议左侧第 20 排 20A。";
const LEADS: &str = "2030-11-08 甲城到乙城 QZ123 06:25";

fn detail(record: &str, other: &str, origin: &str, destination: &str, scheduled: &str) -> String {
    let (date, id, other_date) = if record == TARGET {
        (8, 8123, 7)
    } else {
        (7, 9123, 8)
    };
    format!(
        "Flight Status\nQZ 123\nFixture Airline\nAAA\n{origin}\nBBB\n{destination}\nScheduled\nOn time\nAAA\n{origin}, CN\nFixture Airport\nFlight Departure Times\n{date:02}-Nov-2030\nScheduled\n{scheduled} CST\nActual\n06:30 CST\nBBB\n{destination}, CN\nFlight Arrival Times\n{date:02}-Nov-2030\nScheduled\n09:00 CST\nVIEW FLIGHT DETAILS <https://www.flightstats.com/v2/flight-details/QZ/123?year=2030&month=11&date={date}&flightId={id}>\nFlight Tracker\nPast and Upcoming Flights\nFlights for Friday, {other_date:02}-Nov-2030\n06:25 CST\nAAA\n{origin}\nBBB\n{destination}\n09:00 CST\n<{other}>"
    )
}

async fn go(
    first: Option<(&'static str, String)>,
    second: Option<(&'static str, Result<String, &'static str>)>,
) -> Vec<ServerNotification> {
    let (_home, mut server, _) = setup().await;
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(Arc::new(|_, _| {
        Box::pin(async { Ok(format!("Source 1: Timetable\n{LEADS}\nURL: {OLD}")) })
    }))));
    let mut tool = jeeves_tools::web::fetch_url();
    let first_response = first.clone();
    let second_response = second.clone();
    tool.handler = Arc::new(move |ctx, args| {
        let first = first_response.clone();
        let second = second_response.clone();
        Box::pin(async move {
            let url = args["url"].as_str().unwrap();
            let body = if let Some((_, body)) = first.filter(|(expected, _)| *expected == url) {
                body
            } else {
                let (expected, result) = second.expect("unexpected fetch");
                assert_eq!(url, expected);
                result.map_err(anyhow::Error::msg)?
            };
            let _ = ctx
                .web_receipt
                .unwrap()
                .set(jeeves_tools::web::WebReceipt::Page {
                    requested: url.into(),
                    effective: Some(url.into()),
                    complete: true,
                });
            Ok(body)
        })
    });
    tools.register(tool);
    server.tools = Arc::new(tools);
    let mut plan = Vec::new();
    if first.is_none()
        && let Some((url, _)) = second
    {
        plan.push(Mock::tool_use("verify", "fetch_url", json!({"url":url})));
    }
    plan.push(Mock::text(FORGED));
    let model = Arc::new(Mock::streaming(plan));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });
    let ask = if first.is_some() {
        format!("{ASK}\n供参考的旧页面：{OLD}")
    } else {
        ASK.to_string()
    };
    let events = send(&server, &ask, "dated-flight", None).await.unwrap();
    assert!(server.tasks.list(10).await.unwrap().is_empty());
    let requests = model.received.lock().unwrap();
    assert!(!asked(&requests[0]).contains(TARGET));
    if first.is_some() {
        assert_eq!(
            requests.len(),
            1,
            "the host must do the dated reads before the model replies"
        );
        assert!(
            requests[0]
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .any(|block| {
                    matches!(block, ContentBlock::ToolResult { tool_use_id, content }
                        if tool_use_id.starts_with("preflight-") && content.contains("07-Nov-2030"))
                })
        );
    }
    events
}

fn visible(events: &[ServerNotification]) -> &str {
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ServerNotification::TextDelta(_)))
    );
    let mut replied = events.iter().filter_map(|event| match event {
        ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
        _ => None,
    });
    let answer = replied.next().unwrap();
    assert!(replied.next().is_none());
    for unsupported in ["QZ777", "2030-11-07", "06:30", "20A", "左侧", "20 排"] {
        assert!(!answer.contains(unsupported), "{answer}");
    }
    answer
}

#[tokio::test]
async fn confirms_only_the_target_main_detail_even_with_reverse_date_navigation() {
    let old = detail(OLD, TARGET, "甲城", "乙城", "06:25");
    let target = detail(TARGET, OLD, "Jia Cheng", "Yi Cheng", "06:25");
    let events = go(Some((OLD, old)), Some((TARGET, Ok(target)))).await;
    let fetched = events
        .iter()
        .filter_map(|event| match event {
            ServerNotification::ToolStarted(call) if call.tool == "fetch_url" => {
                call.args["url"].as_str()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(fetched, [OLD, TARGET]);
    assert_eq!(events.iter().filter(|event| matches!(event,
        ServerNotification::ToolCompleted(call) if call.tool == "fetch_url" && call.status == ToolStatus::Ok)).count(), 2);
    let answer = visible(&events);
    for required in [
        "QZ123",
        "2030-11-08",
        "06:25",
        "甲城",
        "乙城",
        TARGET,
        "座位图",
    ] {
        assert!(answer.contains(required), "{required}: {answer}");
    }
    assert!(!answer.contains(OLD));
}

#[tokio::test]
async fn old_detail_and_target_navigation_only_discover_a_lead() {
    let old = detail(OLD, TARGET, "甲城", "乙城", "06:25");
    let second_link = TARGET.replace("8123", "8124");
    let ambiguous = format!("{old}\n06:25 CST\nAAA\n甲城\nBBB\n乙城\n09:10 CST\n<{second_link}>");
    let events = go(Some((OLD, ambiguous)), None).await;
    assert_eq!(events.iter().filter(|event| matches!(event,
        ServerNotification::ToolCompleted(call) if call.tool == "fetch_url" && call.status == ToolStatus::Ok)).count(), 1);
    assert_unconfirmed(&events);
    let events = go(None, Some((TARGET, Ok(old)))).await;
    assert_unconfirmed(&events);
}

#[tokio::test]
async fn dns_failure_overrides_a_streaming_model_guess() {
    let events = go(
        None,
        Some((
            TARGET,
            Err("Public fetch DNS check failed. No page was read."),
        )),
    )
    .await;
    assert!(events.iter().any(|event| matches!(event,
        ServerNotification::ToolCompleted(call) if call.tool_use_id == "verify" && call.status != ToolStatus::Ok && call.output.contains("DNS check failed"))));
    assert_unconfirmed(&events);
}

#[tokio::test]
async fn snippets_wrong_direction_actual_time_and_wrong_record_id_cannot_confirm() {
    assert_unconfirmed(&go(None, None).await);
    for body in [
        detail(TARGET, OLD, "乙城", "甲城", "06:25"),
        detail(TARGET, OLD, "甲城", "乙城", "07:01"),
        detail(TARGET, OLD, "甲城", "乙城", "05:59"),
        detail(TARGET, OLD, "甲城", "乙城", "06:25").replace("flightId=8123>", "flightId=9999>"),
    ] {
        assert_unconfirmed(&go(None, Some((TARGET, Ok(body)))).await);
    }
}

fn assert_unconfirmed(events: &[ServerNotification]) {
    let answer = visible(events);
    assert!(answer.contains("未能核实"), "{answer}");
    assert!(answer.contains("座位图"), "{answer}");
    for unsupported in ["QZ123", "06:25", TARGET, OLD] {
        assert!(!answer.contains(unsupported), "{answer}");
    }
}
