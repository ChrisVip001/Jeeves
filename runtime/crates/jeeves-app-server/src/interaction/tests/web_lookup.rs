//! The IP literal avoids DNS. The injected handler serves both calls, not an HTTP endpoint.

use super::*;
use jeeves_provider::{ContentBlock, mock::Mock};
use jeeves_tools::web::{fetch_url_with_primary, search_web_with_primary};

const ASK: &str = "查一下示例市科技馆现在周六几点闭馆，给我官方来源。";
const QUERY: &str = "示例市科技馆 周六 现行开放公告 闭馆";
const SOURCE: &str = "https://93.184.216.34/fixtures/museum-hours";
const OLD: &str = "旧摘要：周六 18:00 闭馆。";
const CURRENT: &str = "现行官方公告：周六 19:30 闭馆。";

#[tokio::test]
async fn chinese_web_lookup_reads_the_current_page_and_replies_without_creating_a_task() {
    let (_home, mut server, _) = setup().await;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let recorded = calls.clone();
    let fixture: jeeves_tools::Handler = Arc::new(move |_, args| {
        let recorded = recorded.clone();
        Box::pin(async move {
            recorded.lock().unwrap().push(args.clone());
            if args == json!({"query":QUERY,"max_results":5}) {
                Ok(format!("Search result\nURL: {SOURCE}\n{OLD}"))
            } else {
                assert_eq!(args, json!({"url":SOURCE}));
                Ok(CURRENT.into())
            }
        })
    });
    let mut tools = server.tools.without("search_web").without("fetch_url");
    tools.register(search_web_with_primary(Some(fixture.clone())));
    tools.register(fetch_url_with_primary(Some(fixture)));
    server.tools = Arc::new(tools);

    let expected = format!("示例市科技馆周六 19:30 闭馆。 [官方公告]({SOURCE})");
    let model = Arc::new(Mock::streaming(vec![
        Mock::tool_use("lookup-search", "search_web", json!({"query":QUERY})),
        Mock::tool_use("lookup-fetch", "fetch_url", json!({"url":SOURCE})),
        Mock::text(&expected),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });

    let events = send(&server, ASK, "lookup-once", None).await.unwrap();
    let tools: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ServerNotification::ToolCompleted(tool) => {
                assert_eq!(tool.status, ToolStatus::Ok, "{}", tool.output);
                Some(tool.tool.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(tools, ["search_web", "fetch_url"]);
    assert_eq!(
        *calls.lock().unwrap(),
        [
            json!({"query":QUERY,"max_results":5}),
            json!({"url":SOURCE})
        ]
    );

    {
        let rs = model.received.lock().unwrap();
        assert_eq!(rs.len(), 3);
        assert_eq!(asked(&rs[0]), ASK);
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
        assert!(saw(&rs[1], "lookup-search", OLD));
        assert!(!saw(&rs[1], "lookup-search", CURRENT));
        assert!(saw(&rs[2], "lookup-fetch", CURRENT));
    }

    let replies: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            ServerNotification::TurnCompleted(done) => Some(done.reply.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(replies, [expected.as_str()]);
    assert!(server.tasks.list(10).await.unwrap().is_empty());
}
