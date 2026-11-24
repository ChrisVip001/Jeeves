//! Protocol-face tests for `jeeves mcp serve`: handshake, tool list, the behavior of the five
//! memory tools, and "which failures are RPC errors and which are text inside the result".

use serde_json::{Value, json};

use super::server::MemoryServer;

async fn server() -> MemoryServer {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    MemoryServer::new(
        jeeves_state::Facts::new(pool.clone()),
        jeeves_state::Episodes::new(pool),
    )
}

async fn call(server: &MemoryServer, method: &str, params: Value) -> Value {
    server
        .handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))
        .await
        .expect("a request with an id always gets a reply")
}

fn text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn initialize_hands_back_capabilities_and_identity() {
    let server = server().await;
    let response = call(&server, "initialize", json!({})).await;
    let result = &response["result"];
    assert_eq!(result["protocolVersion"], "2025-06-18");
    assert!(
        result["capabilities"]["tools"].is_object(),
        "it has to declare the tools capability"
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "it has to declare the resources capability"
    );
    assert!(
        result["capabilities"]["prompts"].is_object(),
        "it has to declare the prompts capability"
    );
    assert_eq!(result["serverInfo"]["name"], "jeeves-memory");
}

#[tokio::test]
async fn prompts_list_and_get_return_client_instructions_not_memory_writes() {
    let server = server().await;
    let listed = call(&server, "prompts/list", json!({})).await;
    let names: Vec<&str> = listed["result"]["prompts"]
        .as_array()
        .expect("the prompts array")
        .iter()
        .map(|p| p["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(names, vec!["recall", "digest"]);

    let recalled = call(
        &server,
        "prompts/get",
        json!({"name": "recall", "arguments": {"topic": "morning meetings"}}),
    )
    .await;
    let message = &recalled["result"]["messages"][0];
    assert_eq!(message["role"], "user");
    assert!(
        message["content"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("morning meetings"),
        "{recalled}"
    );

    let digest = call(&server, "prompts/get", json!({"name": "digest"})).await;
    assert!(
        digest["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("memory_list")
    );

    let before = text(
        &call(
            &server,
            "tools/call",
            json!({"name": "memory_list", "arguments": {}}),
        )
        .await,
    );
    assert!(
        before.contains("Memory has no facts yet"),
        "resolving a prompt must not write memory: {before}"
    );
}

#[tokio::test]
async fn prompts_get_rejects_a_missing_topic_and_an_unknown_name() {
    let server = server().await;
    let missing = call(&server, "prompts/get", json!({"name": "recall"})).await;
    assert_eq!(missing["error"]["code"], -32602, "{missing}");
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("topic")
    );

    let unknown = call(&server, "prompts/get", json!({"name": "nope"})).await;
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");
}

#[tokio::test]
async fn resources_list_exposes_the_two_read_only_memory_views() {
    let server = server().await;
    let response = call(&server, "resources/list", json!({})).await;
    let uris: Vec<&str> = response["result"]["resources"]
        .as_array()
        .expect("the resources array")
        .iter()
        .map(|r| r["uri"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        uris,
        vec!["jeeves://memory/facts", "jeeves://memory/episodes"]
    );
    assert!(response["error"].is_null(), "{}", response);
}

#[tokio::test]
async fn resources_read_returns_what_was_remembered() {
    let server = server().await;
    call(
        &server,
        "tools/call",
        json!({"name": "memory_remember",
               "arguments": {"subject": "resource-reader", "content": "visible as a resource"}}),
    )
    .await;

    let response = call(
        &server,
        "resources/read",
        json!({"uri": "jeeves://memory/facts"}),
    )
    .await;
    assert!(response["error"].is_null(), "{}", response);
    let contents = response["result"]["contents"]
        .as_array()
        .expect("the contents array");
    assert_eq!(contents.len(), 1);
    assert_eq!(contents[0]["mimeType"], "text/plain");
    assert!(
        contents[0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("visible as a resource"),
        "the fact written through tools must be readable as a resource: {response}"
    );

    let unknown = call(
        &server,
        "resources/read",
        json!({"uri": "jeeves://memory/nope"}),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32602, "{}", unknown);
}

#[tokio::test]
async fn tools_list_exposes_only_the_memory_tools() {
    let server = server().await;
    let response = call(&server, "tools/list", json!({})).await;
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("the tools array")
        .iter()
        .map(|t| t["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec![
            "memory_search",
            "memory_remember",
            "memory_forget",
            "memory_list",
            "memory_episodes"
        ]
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("send") || n.contains("event"))
    );
}

#[tokio::test]
async fn remember_then_search_round_trips_through_the_memory_tables() {
    let server = server().await;
    let saved = call(
        &server,
        "tools/call",
        json!({"name": "memory_remember",
               "arguments": {"subject": "alex", "content": "likes morning meetings"}}),
    )
    .await;
    assert!(text(&saved).contains("Remembered"), "{}", saved);

    let found = call(
        &server,
        "tools/call",
        json!({"name": "memory_search", "arguments": {"query": "alex"}}),
    )
    .await;
    assert!(
        text(&found).contains("likes morning meetings"),
        "what was written has to be findable: {found}"
    );
    assert_eq!(
        found["result"]["isError"],
        Value::Null,
        "this should not be an error"
    );
}

#[tokio::test]
async fn forget_reports_the_count_and_says_when_there_is_nothing() {
    let server = server().await;
    call(
        &server,
        "tools/call",
        json!({"name": "memory_remember", "arguments": {"subject": "bob", "content": "a"}}),
    )
    .await;
    call(
        &server,
        "tools/call",
        json!({"name": "memory_remember", "arguments": {"subject": "bob", "content": "b"}}),
    )
    .await;

    let forgotten = call(
        &server,
        "tools/call",
        json!({"name": "memory_forget", "arguments": {"subject": "bob"}}),
    )
    .await;
    assert!(text(&forgotten).contains("2 facts"), "{}", forgotten);

    let again = call(
        &server,
        "tools/call",
        json!({"name": "memory_forget", "arguments": {"subject": "bob"}}),
    )
    .await;
    assert!(
        text(&again).contains("no memory about"),
        "the second time has to say so plainly: {again}"
    );
}

#[tokio::test]
async fn empty_memory_answers_honestly_not_with_an_error() {
    let server = server().await;
    for (tool, args) in [("memory_list", json!({})), ("memory_episodes", json!({}))] {
        let response = call(
            &server,
            "tools/call",
            json!({"name": tool, "arguments": args}),
        )
        .await;
        assert!(
            response["error"].is_null(),
            "{tool} on an empty store should not be an RPC error"
        );
        assert_eq!(response["result"]["isError"], Value::Null);
        assert!(
            text(&response).contains("Memory has no"),
            "{tool}: {response}"
        );
    }

    let search = call(
        &server,
        "tools/call",
        json!({"name": "memory_search", "arguments": {"query": "???"}}),
    )
    .await;
    assert!(
        text(&search).contains("Nothing in memory matches"),
        "{search}"
    );
}

#[tokio::test]
async fn a_notification_gets_no_response() {
    let server = server().await;
    let response = server
        .handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .await;
    assert!(
        response.is_none(),
        "a notification must not get a response: {response:?}"
    );
}

#[tokio::test]
async fn protocol_errors_and_tool_failures_take_different_paths() {
    let server = server().await;

    let unknown = call(
        &server,
        "tools/call",
        json!({"name": "nope", "arguments": {}}),
    )
    .await;
    assert_eq!(unknown["error"]["code"], -32602, "{unknown}");

    let method = call(&server, "turn/start", json!({})).await;
    assert_eq!(method["error"]["code"], -32601);
    assert!(
        method["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("turn/start")
    );

    let bad_args = call(
        &server,
        "tools/call",
        json!({"name": "memory_search", "arguments": {}}),
    )
    .await;
    assert!(
        bad_args["error"].is_null(),
        "bad arguments should not be an RPC error"
    );
    assert_eq!(bad_args["result"]["isError"], true);
    assert!(text(&bad_args).contains("query"), "{bad_args}");
}
