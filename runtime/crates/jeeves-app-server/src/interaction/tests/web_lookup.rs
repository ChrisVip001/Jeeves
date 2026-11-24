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
        if reply.contains(&self.wrong) {
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
        let stale = format!("{}，[来源]({})。", case.wrong, case.url());
        assert_eq!(
            case.failures(&stale, &case.body, &calls, 0),
            ["missing cited claim", "wrong value"]
        );
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
