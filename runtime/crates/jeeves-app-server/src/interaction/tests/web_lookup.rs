//! IP-literal fixture identifiers avoid DNS; the injected adapter handles both web calls.

use super::*;
use jeeves_provider::{ContentBlock, mock::Mock};
use jeeves_tools::web::{fetch_url_with_primary, search_web_with_primary};
use serde_json::Value;

#[derive(serde::Deserialize)]
struct Case {
    id: String,
    ask: String,
    query: String,
    slug: String,
    excerpt: String,
    body: String,
    evidence: String,
    claim: String,
    wrong: String,
    #[serde(default)]
    other_wrong: Vec<String>,
    #[serde(default)]
    offer: bool,
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!(
        "../../../../../../evals/search/first-answer.json"
    ))
    .unwrap()
}

impl Case {
    fn url(&self) -> String {
        format!("https://93.184.216.34/fixtures/{}", self.slug)
    }

    fn calls(&self) -> [Value; 2] {
        [
            json!({"query":self.query,"max_results":5}),
            json!({"url":self.url()}),
        ]
    }

    fn reply(&self) -> String {
        format!("{}，[官方来源]({})。", self.claim, self.url())
    }

    fn failures(
        &self,
        reply: &str,
        fetched: &str,
        calls: &[Value],
        tasks: usize,
    ) -> Vec<&'static str> {
        let cite = format!("]({})", self.url());
        let mut failures = Vec::new();
        if !reply
            .split(['。', '！', '？', '\n'])
            .any(|clause| clause.contains(&self.claim) && clause.contains(&cite))
        {
            failures.push("missing cited claim");
        }
        if std::iter::once(&self.wrong)
            .chain(&self.other_wrong)
            .any(|wrong| reply.contains(wrong))
        {
            failures.push("wrong value");
        }
        if !fetched.contains(&self.evidence) {
            failures.push("missing fetched evidence");
        }
        if calls != self.calls() {
            failures.push("unexpected web calls");
        }
        if tasks != 0 {
            failures.push("unexpected task");
        }
        failures
    }
}

#[test]
fn first_answer_checks_reject_wrong_detached_unsupported_and_extra_work() {
    for case in cases() {
        let calls = case.calls();
        let reply = case.reply();
        assert!(case.failures(&reply, &case.body, &calls, 0).is_empty());
        for wrong in std::iter::once(&case.wrong).chain(&case.other_wrong) {
            let stale = format!("{wrong}，[来源]({})。", case.url());
            assert_eq!(
                case.failures(&stale, &case.body, &calls, 0),
                ["missing cited claim", "wrong value"]
            );
            let mixed = format!("{reply}另有推荐价：{wrong}。");
            assert_eq!(
                case.failures(&mixed, &case.body, &calls, 0),
                ["wrong value"]
            );
        }
        let wrong_url = reply.replace(
            &case.url(),
            "https://93.184.216.34/fixtures/different-entity",
        );
        let detached = format!("{}。另见：[来源]({})。", case.claim, case.url());
        for wrong in [wrong_url, detached] {
            assert_eq!(
                case.failures(&wrong, &case.body, &calls, 0),
                ["missing cited claim"]
            );
        }
        assert_eq!(
            case.failures(&reply, &case.excerpt, &calls, 0),
            ["missing fetched evidence"]
        );
        assert_eq!(
            case.failures(&reply, &case.body, &calls[..1], 1),
            ["unexpected web calls", "unexpected task"]
        );
    }
}

#[tokio::test]
async fn chinese_search_checks_first_reply_evidence_calls_and_task_count() {
    for case in cases() {
        run_case(case).await;
    }
}

async fn run_case(case: Case) {
    let (_home, mut server, _) = setup().await;
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let calls = recorded.clone();
    let expected = case.calls();
    let source = case.url();
    let excerpt = case.excerpt.clone();
    let body = case.body.clone();
    let adapter: jeeves_tools::Handler = Arc::new(move |_, args| {
        let calls = calls.clone();
        let result = if args == expected[0] {
            format!("Source 1: Search lead\n{excerpt}\nURL: {source}")
        } else {
            assert_eq!(args, expected[1]);
            body.clone()
        };
        Box::pin(async move {
            calls.lock().unwrap().push(args);
            Ok(result)
        })
    });
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(adapter.clone())));
    tools.register(fetch_url_with_primary(Some(adapter)));
    server.tools = Arc::new(tools);
    let model = Arc::new(Mock::streaming(vec![
        Mock::tool_use("search", "search_web", json!({"query":case.query})),
        Mock::tool_use("fetch", "fetch_url", json!({"url":case.url()})),
        Mock::text(&case.reply()),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });
    let events = send(&server, &case.ask, &case.id, None).await.unwrap();
    let web: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ServerNotification::ToolCompleted(call) => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(web.len(), 2);
    assert_eq!(web[0].tool, "search_web");
    assert_eq!(web[1].tool, "fetch_url");
    assert!(web.iter().all(|call| call.status == ToolStatus::Ok));
    {
        let requests = model.received.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(asked(&requests[0]), case.ask);
        if case.offer {
            let fetch = requests[1]
                .tools
                .iter()
                .find(|tool| tool.name == "fetch_url")
                .unwrap();
            for rule in [
                "same offer record",
                "Do not join separate sections",
                "bundles vs single items",
                "checkout total, availability, or unlisted terms",
            ] {
                assert!(
                    fetch.description.contains(rule),
                    "missing offer rule: {rule}"
                );
            }
        }
        let saw = |request: &CreateRequest, id: &str, text: &str| {
            request
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .any(|block| {
                    matches!(block, ContentBlock::ToolResult { tool_use_id, content }
                    if tool_use_id == id && content.contains(text))
                })
        };
        assert!(saw(&requests[1], "search", &case.excerpt));
        assert!(!saw(&requests[1], "search", &case.evidence));
        assert!(saw(&requests[2], "fetch", &case.evidence));
    }
    let replies: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies.len(), 1);
    let tasks = server.tasks.list(10).await.unwrap().len();
    let failures = case.failures(replies[0], &web[1].output, &recorded.lock().unwrap(), tasks);
    assert!(failures.is_empty(), "{}: {failures:?}", case.id);
}

#[tokio::test]
async fn seat_advice_can_search_nearby_without_inventing_a_seat_map() {
    let (_home, mut server, _) = setup().await;
    let ask = "我订了 2030-11-08 早晨六点半从甲城到乙城的飞机（经济舱），哪个靠窗位不被机翼挡？";
    let exact = "甲城 到 乙城 航班 2030-11-08 06:30 起飞 航班号";
    let nearby = "甲城 到 乙城 航班 2030-11-08 06:00 to 07:00 timetable 起飞 航班号";
    let historical = "https://93.184.216.34/fixtures/seat-timetable?day=7&flightId=old";
    let url = "https://93.184.216.34/fixtures/seat-timetable?day=8&flightId=target";
    let focused = "QZ123 甲城 到 乙城 2030-11-08 航班动态 当日实际计划起飞 航司 机场";
    let leads = "甲城到乙城 QZ123 06:25\n2030-11-08 乙城到甲城 QZ456 06:30\n2030-11-09 甲城到乙城 QZ789 06:30";
    let timetable = "2030-11-08 甲城到乙城 QZ123 06:25\n未公布机型或客舱图。";
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::clone(&recorded);
    let adapter: jeeves_tools::Handler = Arc::new(move |_, args| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
            let output = match args["query"].as_str() {
                Some(q) if q == exact && args["nearby_query"].as_str() == Some(nearby) => {
                    format!("Source 1: Timetable\n{leads}\nURL: {historical}")
                }
                Some(q) if q == focused && args.get("nearby_query").is_none() => {
                    format!(
                        "Source 1: Historical status\nURL: {historical}\n2030-11-07 甲城到乙城 QZ123 实际 06:30"
                    )
                }
                None if args["url"].as_str() == Some(historical) => format!(
                    "2030-11-07 实际 06:30\n2030-11-08 当天计划\n甲城到乙城 QZ123 06:25 <{url}>"
                ),
                None if args["url"].as_str() == Some(url) => timetable.to_string(),
                _ => panic!("unexpected fixture lookup"),
            };
            calls.lock().unwrap().push(args);
            Ok(output)
        })
    });
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(adapter.clone())));
    tools.register(fetch_url_with_primary(Some(adapter)));
    server.tools = Arc::new(tools);
    let reply = format!(
        "2030-11-08 甲城到乙城的候选是 [QZ123，06:25]({url})，比你记的早五分钟。没有核实机型和座位图，先不猜排号。"
    );
    let model = Arc::new(Mock::streaming(vec![
        Mock::tool_use("read-old", "fetch_url", json!({"url":historical})),
        Mock::tool_use("verify", "fetch_url", json!({"url":url})),
        Mock::text(&format!("{reply}建议选左侧第 20 排（20A）。")),
        Mock::text("这是外部会话，不执行查询。"),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });
    let events = send(&server, ask, "nearby-flight", None).await.unwrap();
    {
        let requests = model.received.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let has_text = |index: usize, needle: &str| {
            requests[index]
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .any(|block| {
                    matches!(block,
                ContentBlock::ToolResult { content, .. } if content.contains(needle))
                })
        };
        assert!(!has_text(0, url));
        assert!(has_text(1, &format!("QZ123 06:25 <{url}>")));
        assert!(has_text(2, "2030-11-08 甲城到乙城 QZ123 06:25"));
        assert!(requests[0].messages.iter().any(|m| m.text().ends_with(ask)));
        assert!(requests[0].tools.iter().any(|t| t.name == "search_web"));
        let prompt = requests[0].system_text().unwrap();
        for rule in [
            "even when phrased as advice",
            "nearby time window",
            "same date and direction",
            "explicitly keeping the date",
            "a candidate, not as the user's booking",
            "Never combine a flight number from one row with the time from another",
            "Do not relax a hard time limit",
            "do not guess row numbers",
            "successfully fetched dated flight tracker detail",
            "A verified public flight is still not confirmation of the user's booking.",
        ] {
            assert!(prompt.contains(rule), "{rule}");
        }
        assert!(
            requests[0]
                .messages
                .iter()
                .flat_map(|m| &m.content)
                .any(|b| {
                    matches!(b, ContentBlock::ToolResult { tool_use_id, content }
                if tool_use_id.starts_with("schedule-") && content.contains("QZ123 06:25"))
                })
        );
        let refinement_result = requests[0]
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } if content.contains("Historical status") => Some(tool_use_id),
                _ => None,
            })
            .expect("dated refinement runs before the first model call");
        assert!(refinement_result.starts_with("schedule-") && refinement_result.ends_with("-2"));
    }
    assert_eq!(
        *recorded.lock().unwrap(),
        [
            json!({"query":exact,"max_results":4,"nearby_query":nearby}),
            json!({"query":focused,"max_results":3}),
            json!({"url":historical}),
            json!({"url":url})
        ]
    );
    let done = events
        .iter()
        .find_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
            _ => None,
        })
        .unwrap();
    assert!(done.contains("未能核实"));
    assert!(done.contains("航司值机页的座位图"));
    for unsupported in ["QZ123", "QZ456", "QZ789", "左侧", "20A", "20 排"] {
        assert!(!done.contains(unsupported));
    }
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ServerNotification::TextDelta(_)))
    );
    assert!(
        matches!(events.iter().find(|e| matches!(e, ServerNotification::ToolStarted(_))),
        Some(ServerNotification::ToolStarted(started)) if started.tool == "search_web")
    );
    let replay = send(&server, ask, "nearby-flight", None).await.unwrap();
    assert!(
        !replay
            .iter()
            .any(|e| matches!(e, ServerNotification::ToolStarted(_)))
    );
    assert_eq!(recorded.lock().unwrap().len(), 4);
    let (sink, mut frames) = EventSink::channel();
    start(
        &server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id: "nearby-flight-external".into(),
            message: ask.into(),
            task_id: None,
            cwd: None,
            attachments: None,
            source: Some(jeeves_protocol::InteractionSource {
                channel: "lark".into(),
                chat_id: "chat".into(),
                message_id: "guest-message".into(),
                sender_id: "guest".into(),
            }),
        },
        RequestId::Number(4),
        &sink,
    )
    .await
    .unwrap();
    while let Ok(frame) = frames.try_recv() {
        assert!(!matches!(
            frame,
            Frame::Notification(ServerNotification::ToolStarted(_))
        ));
    }
    assert_eq!(recorded.lock().unwrap().len(), 4);
    assert!(
        model
            .received
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .tools
            .is_empty()
    );
    assert!(server.tasks.list(10).await.unwrap().is_empty());
}

mod dated_status;
