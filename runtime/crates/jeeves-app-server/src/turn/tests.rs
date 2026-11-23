//! Assembly tests for one turn — this layer answers exactly one question: **how did this turn run**.
//!
//! The model is a scripted fake (`jeeves_provider::mock`); "what the model said on which call" is fully
//! controllable, so every branch of a turn can be pinned. After a run, two things are checked:
//!
//!   1. which notifications the client received;
//!   2. **how many times the model was actually called** — the cost of a turn is that number.

use std::sync::Arc;

use jeeves_config::Settings;
use jeeves_protocol::{
    ErrorObject, JsonRpcMessage, JsonRpcRequest, RequestId, ServerNotification, TurnStartParams,
};
use jeeves_provider::mock::Mock;
use jeeves_provider::{CreateRequest, Resolved};
use serde_json::json;
use tokio::sync::mpsc;

use crate::{EventSink, Frame, Server};

struct Ran {
    notifications: Vec<ServerNotification>,
    requests: Vec<CreateRequest>,
    error: Option<ErrorObject>,
}

async fn server(mock: Arc<Mock>) -> Server {
    let dir = tempfile::tempdir().expect("temporary directory");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let settings = Settings {
        home: dir.path().to_path_buf(),
        ..Settings::default()
    };
    let _ = dir.keep();

    let server = Server::boot(pool, settings).await;
    server.set_resolved(Some(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: mock,
        model: "test-model".to_string(),
        small_model: "test-small".to_string(),
    }));
    server
}

#[tokio::test]
async fn a_model_request_fits_its_output_into_the_remaining_context() {
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use("material", "read_file", json!({"path": "material.txt"})),
        Mock::text("Reviewed."),
    ]));
    let mut server = server(mock.clone()).await;
    server.restrict_tools(&["read_file"]);
    let mut resolved = server.resolved().unwrap();
    resolved.context_window = 8_192;
    server.set_resolved(Some(resolved));
    let home = server.settings().home;
    std::fs::write(
        home.join("material.txt"),
        "A line of evidence for the review.\n".repeat(200),
    )
    .unwrap();
    let (sink, _) = EventSink::channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            message: "Review this material. ".repeat(240),
            cwd: Some(home.display().to_string()),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap();
    let requests = mock.received.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].max_tokens < requests[0].max_tokens);
    for request in requests.iter() {
        let input = jeeves_provider::tokens::estimate_messages(&request.messages)
            + jeeves_provider::tokens::estimate_text(&request.system_text().unwrap_or_default())
            + jeeves_provider::tokens::estimate_tools(&request.tools);
        assert!(request.max_tokens > 0);
        assert!((request.max_tokens as usize) + input < 8_192);
    }
}

#[tokio::test]
async fn delegated_model_requests_use_the_same_output_policy() {
    let mock = Arc::new(Mock::new(vec![
        Mock::tool_use(
            "child",
            "delegate_task",
            json!({"task": "Return a short finding."}),
        ),
        Mock::text("The child completed the review."),
        Mock::text("The review is complete."),
    ]));
    let server = server(mock.clone()).await;
    {
        let mut settings = server.settings.write().unwrap();
        settings.delegate_enabled = true;
        settings.context_window = Some(8192);
    }
    let (sink, _) = EventSink::channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            session_id: Some("delegate-test".into()),
            message: "Delegate a short review.".into(),
            ..Default::default()
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .unwrap();
    let requests = mock.received.lock().unwrap();
    let child = requests
        .iter()
        .find(|request| {
            request
                .system_text()
                .is_some_and(|system| system.contains("You are a subagent:"))
        })
        .expect("the child model was called");
    assert!(child.max_tokens > 2048);
    let input = jeeves_provider::tokens::estimate_messages(&child.messages)
        + jeeves_provider::tokens::estimate_text(&child.system_text().unwrap_or_default())
        + jeeves_provider::tokens::estimate_tools(&child.tools);
    assert!((child.max_tokens as usize) + input < 8192);
}

async fn call(server: &Server, request: JsonRpcRequest) -> JsonRpcMessage {
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::handle(server, request, EventSink::new(tx)).await;
    let mut last = None;
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Response(message) = frame {
            last = Some(message);
        }
    }
    last.expect("every request has a reply")
}

fn request(id: i32, method: &str, params: serde_json::Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: RequestId::Number(id),
        method: method.to_string(),
        params: Some(params),
    }
}

async fn run(mock: Mock, message: &str) -> Ran {
    let mock = Arc::new(mock);
    let server = server(mock.clone()).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);

    let outcome = crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: message.to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await;

    let mut notifications = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        match frame {
            Frame::Notification(notification) => notifications.push(notification),
            Frame::Response(_) => {}
        }
    }

    let requests = mock.received.lock().expect("lock").clone();
    Ran {
        notifications,
        requests,
        error: outcome.err(),
    }
}

#[tokio::test]
async fn memory_index_failure_does_not_block_the_turn() {
    let mock = Arc::new(Mock::new(vec![Mock::text("ordinary reply")]));
    let server = server(mock.clone()).await;
    let message = "fixture memory lookup";
    server
        .episodes
        .add("2026-11-19", "fixture memory lookup episode")
        .await
        .unwrap();
    sqlx::query("DROP TABLE facts_fts")
        .execute(&server.pool)
        .await
        .unwrap();
    assert!(server.facts.search(message, 4).await.is_err());
    assert!(
        jeeves_memory::retrieve_context(&server.facts, &server.episodes, message, 4)
            .await
            .unwrap()
            .contains("fixture memory lookup episode"),
        "fact lookup failure must not discard available episodes"
    );
    sqlx::query("DROP TABLE episodes_fts")
        .execute(&server.pool)
        .await
        .unwrap();
    assert!(
        jeeves_memory::retrieve_context(&server.facts, &server.episodes, message, 4)
            .await
            .is_err()
    );
    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            session_id: Some("memory-failure".into()),
            message: message.into(),
            stream: Some(true),
            ..TurnStartParams::default()
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .unwrap();
    let mut completed = false;
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Notification(ServerNotification::TurnCompleted(event)) = frame {
            assert_eq!(event.reply, "ordinary reply");
            completed = true;
        }
    }
    assert!(
        completed,
        "a failed index must not prevent normal turn completion"
    );
    assert_eq!(mock.received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_failed_episode_lookup_keeps_available_facts() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    server
        .facts
        .add("fixture", "fixture recall", "user", "user")
        .await
        .unwrap();
    sqlx::query("DROP TABLE episodes_fts")
        .execute(&server.pool)
        .await
        .unwrap();
    let text = jeeves_memory::retrieve_context(&server.facts, &server.episodes, "fixture", 4)
        .await
        .unwrap();
    assert!(text.contains("fixture recall"));
}

#[tokio::test]
async fn a_broken_mcp_server_still_leaves_the_builtin_tools() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    std::fs::write(
        dir.path().join("mcp.json"),
        json!({"servers": [{"name": "ghost", "command": "/definitely/not/a/binary"}]}).to_string(),
    )
    .expect("write the config");

    let settings = Settings {
        home: dir.path().to_path_buf(),
        ..Settings::default()
    };
    let server = Server::boot(pool, settings).await;

    let names = server.tools.names();
    assert!(names.contains(&"memory"), "{names:?}");
    assert!(
        !names.iter().any(|name| name.starts_with("ghost")),
        "an unreachable server must not leave half a tool behind: {names:?}"
    );
}

#[tokio::test]
async fn an_mcp_server_next_to_home_lands_in_the_tool_table() {
    let Some(python) = python3() else {
        eprintln!("skipped: no python3 on this machine");
        return;
    };
    let dir = tempfile::tempdir().expect("temporary directory");
    let script = dir.path().join("fake_mcp_server.py");
    std::fs::write(&script, FAKE_SERVER).expect("write the server");
    std::fs::write(
        dir.path().join("mcp.json"),
        json!({"servers": [{"name": "demo", "command": python, "args": [script]}]}).to_string(),
    )
    .expect("write the config");

    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let settings = Settings {
        home: dir.path().to_path_buf(),
        ..Settings::default()
    };
    let server = Server::boot(pool, settings).await;

    let names = server.tools.names();
    assert!(
        names.contains(&"demo_echo"),
        "MCP tools enter the table with their server name as prefix: {names:?}"
    );
    assert!(
        names.contains(&"find_tools"),
        "the server's own tool is still there: {names:?}"
    );
}

fn python3() -> Option<String> {
    let out = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()?;
    out.status.success().then(|| "python3".to_string())
}

const FAKE_SERVER: &str = r#"
import json, sys

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue
    if msg.get("method") == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fake", "version": "1"}}
    elif msg.get("method") == "tools/list":
        result = {"tools": [{"name": "echo",
                             "inputSchema": {"type": "object", "properties": {}}}]}
    else:
        send({"jsonrpc": "2.0", "id": msg["id"],
              "error": {"code": -32601, "message": "no such method"}})
        continue
    send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
"#;

#[tokio::test]
async fn tool_started_goes_out_before_tool_completed() {
    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        ),
        Mock::text("noted."),
    ]);
    let ran = run(mock, "remind me of something").await;

    assert!(ran.error.is_none(), "{:?}", ran.error);
    let order: Vec<&str> = ran
        .notifications
        .iter()
        .filter_map(|n| match n {
            ServerNotification::ToolStarted(_) => Some("started"),
            ServerNotification::ToolCompleted(_) => Some("completed"),
            _ => None,
        })
        .collect();
    assert_eq!(
        order,
        vec!["started", "completed"],
        "ToolStarted has to come before ToolCompleted, each exactly once"
    );
}

#[tokio::test]
async fn the_write_lock_is_lazy_but_stable_per_session() {
    let server = server(Arc::new(Mock::new(vec![]))).await;

    let first = server.session_write_lock("s1");
    let again = server.session_write_lock("s1");
    let other = server.session_write_lock("s2");

    assert!(Arc::ptr_eq(&first, &again), "one lock per session");
    assert!(
        !Arc::ptr_eq(&first, &other),
        "different sessions get different locks"
    );
}

#[tokio::test]
async fn a_held_write_lock_blocks_another_acquirer() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    let lock = server.session_write_lock("s");

    let guard = lock.lock().await;
    let attempt = lock.clone();
    let blocked = tokio::time::timeout(std::time::Duration::from_millis(50), async move {
        let _ = attempt.lock().await;
    })
    .await;
    assert!(
        blocked.is_err(),
        "while the write lock is held another taker has to wait"
    );

    drop(guard);
    let relock = lock.clone();
    let acquired = tokio::time::timeout(std::time::Duration::from_millis(50), async move {
        let guard = relock.lock().await;
        drop(guard);
    })
    .await;
    assert!(
        acquired.is_ok(),
        "after the release the next taker gets in at once"
    );
}

#[tokio::test]
async fn interrupt_hits_a_running_turn_and_misses_a_finished_one() {
    let mock = Arc::new(Mock::new(vec![]));
    let server = server(mock).await;

    let response = call(
        &server,
        request(1, "turn/interrupt", json!({"turnId": "t_missing"})),
    )
    .await;
    match response {
        JsonRpcMessage::Response(resp) => {
            assert_eq!(resp.result, json!({"interrupted": false}))
        }
        other => panic!("interrupting a turn that does not exist should not error: {other:?}"),
    }

    let (interrupt, _) = server
        .begin_turn("t_live", "default")
        .expect("the session is idle");
    let response = call(
        &server,
        request(2, "turn/interrupt", json!({"turnId": "t_live"})),
    )
    .await;
    match response {
        JsonRpcMessage::Response(resp) => {
            assert_eq!(resp.result, json!({"interrupted": true}))
        }
        other => panic!("interrupting a running turn should not error: {other:?}"),
    }
    assert!(
        interrupt.is_cancelled(),
        "the token has to be cancelled for real"
    );
    server.finish_turn("t_live");
}

#[tokio::test]
async fn config_write_lands_in_memory_and_survives_a_reboot() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    let settings = Settings {
        home: home.clone(),
        ..Settings::default()
    };
    let server = Server::boot(pool, settings).await;

    let response = call(
        &server,
        request(
            1,
            "config/write",
            json!({"patch": {"historyTurns": 20, "maxTokens": 4096}}),
        ),
    )
    .await;
    match response {
        JsonRpcMessage::Response(resp) => {
            assert_eq!(resp.result["config"]["historyTurns"], 20);
            assert_eq!(resp.result["config"]["maxTokens"], 4096);
        }
        other => panic!("config/write should not error: {other:?}"),
    }
    assert_eq!(server.settings().history_turns, 20);
    assert_eq!(server.settings().max_tokens, 4096);
    assert!(
        home.join("settings.json").exists(),
        "the patch has to reach the disk"
    );

    let settings2 = Settings {
        home,
        ..Settings::default()
    };
    let server2 = crate::open(&settings2).await.expect("reopen");
    assert_eq!(server2.settings().history_turns, 20);
    assert_eq!(server2.settings().max_tokens, 4096);
}

#[tokio::test]
async fn config_write_cannot_smuggle_a_model_or_a_key_in() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    let server = Server::boot(
        pool,
        Settings {
            home: home.clone(),
            ..Settings::default()
        },
    )
    .await;

    let response = call(
        &server,
        request(
            1,
            "config/write",
            json!({"patch": {"provider": "deepseek", "model": "deepseek-flash", "apiKey": "sk-leaked-in"}}),
        ),
    )
    .await;
    match response {
        JsonRpcMessage::Response(resp) => {
            assert_eq!(resp.result["config"]["provider"], "anthropic");
        }
        other => panic!("should not error: {other:?}"),
    }
    assert_eq!(server.settings().provider, "anthropic");
    assert_eq!(server.settings().model, None);
    assert_eq!(server.settings().api_key, None);
    let saved = std::fs::read_to_string(home.join("settings.json")).unwrap_or_default();
    assert!(
        !saved.contains("sk-"),
        "a key must never reach settings.json: {saved}"
    );
}

#[tokio::test]
async fn a_bad_patch_is_rejected_without_touching_anything() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    let settings = Settings {
        home: home.clone(),
        ..Settings::default()
    };
    let server = Server::boot(pool, settings).await;

    let response = call(
        &server,
        request(1, "config/write", json!({"patch": {"historyTurns": -1}})),
    )
    .await;
    match response {
        JsonRpcMessage::Error(error) => {
            assert_eq!(error.error.code, jeeves_protocol::codes::INVALID_PARAMS);
            assert!(error.error.message.contains("historyTurns"));
        }
        other => panic!("an illegal patch must be refused: {other:?}"),
    }
    assert_eq!(
        server.settings().history_turns,
        jeeves_config::Settings::default().history_turns
    );
    assert!(!home.join("settings.json").exists());
}

#[tokio::test]
async fn model_list_reports_the_provider_catalog() {
    let mock = Arc::new(Mock::new(vec![]));
    let server = server(mock).await;

    let response = call(&server, request(1, "model/list", json!({}))).await;
    match response {
        JsonRpcMessage::Response(resp) => {
            let data = resp.result["data"].as_array().expect("the data array");
            assert_eq!(data.len(), jeeves_provider::PROVIDERS.len());
            assert_eq!(data[0]["provider"], "anthropic");
            assert_eq!(data[0]["flagship"], "claude-sonnet-5-5");
        }
        other => panic!("model/list should not error: {other:?}"),
    }

    let response = call(
        &server,
        request(2, "model/list", json!({"provider": "deepseek"})),
    )
    .await;
    match response {
        JsonRpcMessage::Response(resp) => {
            let data = resp.result["data"].as_array().expect("the data array");
            assert_eq!(data.len(), 1);
            assert_eq!(data[0]["provider"], "deepseek");
        }
        other => panic!("model/list with a filter should not error: {other:?}"),
    }
}

#[tokio::test]
async fn a_turn_leaves_a_trace_behind() {
    let mock = Mock::new(vec![Mock::text("Four.")]);
    let server = server(Arc::new(mock)).await;
    let home = server.settings().home.clone();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    crate::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "What is 2+2?".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect("this turn should get through");

    let mut traces = std::fs::read_dir(home.join("traces"))
        .expect("a traces directory")
        .collect::<Vec<_>>();
    assert_eq!(traces.len(), 1, "one file per turn: {:?}", traces);
    let trace = std::fs::read_to_string(traces.remove(0).unwrap().path()).unwrap();
    let record: serde_json::Value = serde_json::from_str(trace.trim()).expect("valid JSON");
    assert_eq!(record["userMessage"], "What is 2+2?");
    assert_eq!(record["meta"]["model"], "test-model");
    assert_eq!(
        record["meta"]["usage"]["outputTokens"], 5,
        "usage lands on disk with meta"
    );

    assert!(!home.join("usage.jsonl").exists());
}

#[tokio::test]
async fn a_context_overflow_is_compacted_and_retried_exactly_once() {
    let mock = Arc::new(Mock::with_outcomes(vec![
        Mock::context_overflow(),
        Ok(Mock::text("a summary of the earlier conversation.")),
        Ok(Mock::text("Four.")),
    ]));
    let server = server(mock.clone()).await;
    for i in 0..4 {
        server
            .chat
            .append_exchange(
                &format!("question {i}"),
                &format!("answer {i}"),
                "test",
                "cli",
                None,
            )
            .await
            .expect("write the session");
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "What is 2+2?".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect("the retry after the overflow should get through");

    let mut reply = String::new();
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Notification(ServerNotification::TurnCompleted(done)) = frame {
            reply = done.reply.clone();
        }
    }
    assert_eq!(reply, "Four.");
    assert_eq!(
        mock.received.lock().expect("lock").len(),
        3,
        "gate + overflow + summary + retry, not one call extra"
    );
    assert!(
        server
            .chat
            .load_rollup("test")
            .await
            .expect("query the store")
            .is_some(),
        "a forced compaction has to leave a summary"
    );
}

#[tokio::test]
async fn precompaction_prepares_the_summary_once() {
    let mock = Arc::new(Mock::with_outcomes(vec![Ok(Mock::text(
        "a summary of the earlier conversation.",
    ))]));
    let server = server(mock.clone()).await;
    server.settings.write().unwrap().compact_threshold = 0.4;
    let long = "word ".repeat(600);
    for i in 0..12 {
        server
            .chat
            .append_exchange(
                &format!("q{i} {long}"),
                &format!("a{i} {long}"),
                "test",
                "cli",
                None,
            )
            .await
            .expect("write the session");
    }
    let resolved = server.resolved().expect("a model is configured");
    crate::turn::history::precompact(&server, &resolved, "test").await;
    assert!(
        server
            .chat
            .load_rollup("test")
            .await
            .expect("query")
            .is_some(),
        "the background pass leaves a summary"
    );
    let calls = mock.received.lock().expect("lock").len();
    crate::turn::history::precompact(&server, &resolved, "test").await;
    assert_eq!(
        mock.received.lock().expect("lock").len(),
        calls,
        "nothing new to fold"
    );
}

#[tokio::test]
async fn a_plain_provider_error_is_not_retried() {
    let mock = Arc::new(Mock::with_outcomes(vec![Err(
        jeeves_provider::ProviderError::Api("boom".to_string()),
    )]));
    let server = server(mock.clone()).await;
    let (tx, _rx) = mpsc::unbounded_channel();

    let error = crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "hello".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect_err("it has to report honestly");

    assert_eq!(error.code, jeeves_protocol::codes::PROVIDER_ERROR);
    assert_eq!(
        mock.received.lock().expect("lock").len(),
        1,
        "one for the gate, one for the loop, no third model call"
    );
}

async fn drain_request(socket: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;

    let mut seen: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                seen.extend_from_slice(&chunk[..read]);
                let Some(head_end) = seen.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let head = String::from_utf8_lossy(&seen[..head_end]).to_lowercase();
                let want: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                if seen.len() >= head_end + 4 + want {
                    return;
                }
            }
        }
    }
}

async fn scripted_http(responses: Vec<(u16, String)>) -> String {
    use tokio::io::AsyncWriteExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the port");
    let addr = listener.local_addr().expect("the local address");
    tokio::spawn(async move {
        let mut index = 0usize;
        while let Ok((mut socket, _)) = listener.accept().await {
            drain_request(&mut socket).await;
            let (status, body) = responses
                .get(index)
                .cloned()
                .unwrap_or((200, "{}".to_string()));
            index += 1;
            let reason = if status == 429 {
                "Too Many Requests"
            } else {
                "OK"
            };
            let extra = if status == 429 {
                "Retry-After: 0\r\n"
            } else {
                ""
            };
            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.flush().await;
        }
    });
    format!("http://{addr}")
}

fn completion(text: &str) -> String {
    serde_json::json!({"status":"completed", "output":[{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":text}]}], "usage":{"input_tokens":1,"output_tokens":1}}).to_string()
}

fn sse_text(text: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\n",
        serde_json::json!({"type":"response.output_text.delta", "delta":text}),
        serde_json::json!({"type":"response.completed", "response":serde_json::from_str::<serde_json::Value>(&completion(text)).unwrap()})
    )
}

#[tokio::test]
async fn a_rate_limited_call_is_retried_and_reported_in_the_meta() {
    let base = scripted_http(vec![
        (429, r#"{"error":{"message":"slow down"}}"#.to_string()),
        (200, sse_text("Four.")),
    ])
    .await;

    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        provider: "openai".to_string(),
        api_key: Some("test-key".to_string()),
        base_url: Some(base),
        model: Some("test-model".to_string()),
        small_model: Some("test-model".to_string()),
        max_tokens: 256,
        ..Default::default()
    };
    let server = crate::open(&settings)
        .await
        .expect("assemble with the real client");

    let (tx, mut rx) = mpsc::unbounded_channel();
    crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "What is 2+2?".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &EventSink::new(tx),
    )
    .await
    .expect("after the rate limit it should finish as usual");

    let mut retries = 0;
    let mut meta_retries = -1;
    while let Ok(frame) = rx.try_recv() {
        if let Frame::Notification(notification) = frame {
            match notification {
                ServerNotification::Retry(retry) => {
                    retries += 1;
                    assert!(retry.reason.contains("429"), "{retry:?}");
                    assert!(
                        !retry.turn_id.is_empty(),
                        "the notification has to name the turn"
                    );
                }
                ServerNotification::TurnCompleted(done) => {
                    meta_retries = done.meta.retries;
                }
                _ => {}
            }
        }
    }
    assert_eq!(retries, 1, "every retry owes a notification");
    assert_eq!(
        meta_retries, 1,
        "the count in meta is measured, not guessed"
    );
}

#[test]
fn turn_cwd_validation_is_fail_closed() {
    use crate::turn::resolve_turn_cwd;

    let dir = tempfile::tempdir().expect("temporary directory");
    let ok = resolve_turn_cwd(&Some(dir.path().display().to_string()));
    assert!(
        ok.unwrap().is_some(),
        "an absolute path that exists should pass"
    );

    assert!(resolve_turn_cwd(&None).unwrap().is_none());
    assert!(resolve_turn_cwd(&Some(String::new())).unwrap().is_none());
    assert!(resolve_turn_cwd(&Some("   ".into())).unwrap().is_none());

    let err = resolve_turn_cwd(&Some("relative/path".into())).unwrap_err();
    assert!(err.message.contains("absolute path"), "{}", err.message);

    let err = resolve_turn_cwd(&Some("/no/such/directory/jeeves".into())).unwrap_err();
    assert!(err.message.contains("does not exist"), "{}", err.message);
}

async fn run_with_attachments(
    mock: Mock,
    vision: bool,
    message: &str,
    attachments: Option<Vec<jeeves_protocol::TurnAttachment>>,
    prepare: impl FnOnce(&std::path::Path),
) -> Ran {
    let mock = Arc::new(mock);
    let dir = tempfile::tempdir().expect("temporary directory");
    prepare(dir.path());
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let settings = Settings {
        home: dir.path().to_path_buf(),
        ..Settings::default()
    };
    let _ = dir.keep();
    let server = Server::boot(pool, settings).await;
    server.set_resolved(Some(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision,
        provider_id: "mock".to_string(),
        client: mock.clone(),
        model: "test-model".to_string(),
        small_model: "test-small".to_string(),
    }));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);
    let outcome = crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments,
            session_id: Some("test".to_string()),
            message: message.to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await;
    let mut notifications = Vec::new();
    while let Ok(frame) = rx.try_recv() {
        match frame {
            Frame::Response(_) => {}
            Frame::Notification(notification) => notifications.push(notification),
        }
    }
    let requests = mock.received.lock().expect("lock").clone();
    Ran {
        notifications,
        requests,
        error: outcome.err(),
    }
}

fn png_attachment(path: &str) -> Option<Vec<jeeves_protocol::TurnAttachment>> {
    Some(vec![jeeves_protocol::TurnAttachment {
        path: path.to_string(),
        media_type: None,
    }])
}

#[tokio::test]
async fn an_attachment_reaches_the_provider_as_an_image_block() {
    let png = {
        let mut bytes = vec![0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&13u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes
    };
    let ran = run_with_attachments(
        Mock::new(vec![Mock::text("the image is a colour patch.")]),
        true,
        "what is in this image?",
        png_attachment("uploads/px.png"),
        |home| {
            let dir = home.join("uploads");
            std::fs::create_dir_all(&dir).expect("create the upload directory");
            std::fs::write(dir.join("px.png"), png.clone()).expect("place the image");
        },
    )
    .await;
    assert!(ran.error.is_none(), "{:?}", ran.error);
    let image = ran
        .requests
        .iter()
        .flat_map(|r| r.messages.iter())
        .filter(|m| matches!(m.role, jeeves_provider::Role::User))
        .flat_map(|m| m.content.iter())
        .find_map(|b| match b {
            jeeves_provider::ContentBlock::Image { media_type, data } => {
                Some((media_type.as_str(), data.as_str()))
            }
            _ => None,
        })
        .expect("the user message carries an Image block");
    assert_eq!(
        image.0, "image/png",
        "the type comes from the extension sniff"
    );
    use base64::Engine as _;
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(image.1)
            .expect("valid base64"),
        png,
        "the bytes reach the provider unchanged"
    );
    let with_text = ran
        .requests
        .iter()
        .flat_map(|r| r.messages.iter())
        .filter(|m| matches!(m.role, jeeves_provider::Role::User))
        .any(|m| m.text().contains("what is in this image?"));
    assert!(
        with_text,
        "the user's text rides into the request with the image"
    );
}

#[tokio::test]
async fn an_oversized_image_is_refused_with_its_real_dimensions() {
    let ran = run_with_attachments(
        Mock::new(vec![]),
        true,
        "have a look at this long image",
        png_attachment("uploads/long.png"),
        |home| {
            let mut bytes = vec![0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
            bytes.extend_from_slice(&13u32.to_be_bytes());
            bytes.extend_from_slice(b"IHDR");
            bytes.extend_from_slice(&1120u32.to_be_bytes());
            bytes.extend_from_slice(&11_100u32.to_be_bytes());
            bytes.extend_from_slice(&[8, 6, 0, 0, 0]);
            let dir = home.join("uploads");
            std::fs::create_dir_all(&dir).expect("create the upload directory");
            std::fs::write(dir.join("long.png"), bytes).expect("place the image");
        },
    )
    .await;
    let error = ran
        .error
        .as_ref()
        .expect("over the per-side limit it has to stop at the door");
    assert!(
        error.message.contains("1120×11100"),
        "the error has to give the real size: {}",
        error.message
    );
    assert!(
        error.message.contains("in pieces"),
        "the error has to give an actionable next step: {}",
        error.message
    );
    assert!(
        ran.requests.is_empty(),
        "a failure must not burn a model call"
    );
}

#[tokio::test]
async fn attachments_fail_up_front_on_a_non_vision_provider() {
    let ran = run_with_attachments(
        Mock::new(vec![]),
        false,
        "have a look at this image",
        png_attachment("uploads/px.png"),
        |_| {},
    )
    .await;
    let error = ran
        .error
        .as_ref()
        .expect("with vision off it has to stop at the door");
    assert!(
        error.message.contains("看不了图") && error.message.contains("多模态模型"),
        "{}",
        error.message
    );
    assert!(
        ran.requests.is_empty(),
        "a failure must not burn a model call"
    );
}

#[tokio::test]
async fn an_attachment_outside_home_is_refused_before_the_turn() {
    let ran = run_with_attachments(
        Mock::new(vec![]),
        true,
        "have a look at this image",
        png_attachment("../escape.png"),
        |home| {
            std::fs::write(
                home.parent()
                    .expect("a tempdir always has a parent")
                    .join("escape.png"),
                [0u8; 8],
            )
            .expect("place the escaping file");
        },
    )
    .await;
    let error = ran
        .error
        .as_ref()
        .expect("a path outside the workspace must be refused");
    assert!(error.message.contains("outside home"), "{}", error.message);
    assert!(ran.requests.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_attachment_outside_home_is_refused() {
    let ran = run_with_attachments(
        Mock::new(vec![]),
        true,
        "have a look at this image",
        png_attachment("uploads/escape.png"),
        |home| {
            let dir = home.join("uploads");
            std::fs::create_dir_all(&dir).expect("create the upload directory");
            std::os::unix::fs::symlink("/etc/hosts", dir.join("escape.png"))
                .expect("create the escaping symlink");
        },
    )
    .await;
    let error = ran
        .error
        .as_ref()
        .expect("a path outside the workspace must be refused");
    assert!(error.message.contains("outside home"), "{}", error.message);
    assert!(ran.requests.is_empty());
}

#[tokio::test]
async fn the_session_working_directory_is_used_when_the_turn_names_none() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    let cwd = std::env::temp_dir();
    let cwd_text = cwd.to_string_lossy().into_owned();
    server
        .chat
        .create_session("s1", None, Some(cwd_text.as_str()))
        .await
        .expect("create the session");

    assert_eq!(
        crate::turn::session_cwd(&server, "s1").await.as_deref(),
        Some(cwd.as_path()),
        "the directory the session remembers is this turn's writable root"
    );
    assert_eq!(
        crate::turn::session_cwd(&server, "s2").await,
        None,
        "with no such session, no directory may be invented"
    );
    assert_eq!(
        crate::turn::session_cwd(&server, "default").await,
        None,
        "a session that never chose a directory has none (writes are refused, not dropped into the process cwd)"
    );
}

#[tokio::test]
async fn a_vanished_session_directory_is_not_silently_replaced() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    let work = tempfile::tempdir().expect("temporary directory");
    let path = work.path().to_string_lossy().to_string();
    server
        .chat
        .create_session("s1", None, Some(&path))
        .await
        .expect("create the session");
    assert!(crate::turn::session_cwd(&server, "s1").await.is_some());

    drop(work);
    assert_eq!(
        crate::turn::session_cwd(&server, "s1").await,
        None,
        "the directory is gone, so there is no writable scope — another directory must not stand in"
    );
}

#[tokio::test]
async fn turn_list_reports_only_what_is_still_running() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    let _token = server.begin_turn("t1", "s1").expect("the session is idle");

    let all = server.active_turns(None);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].0, "t1");
    assert_eq!(all[0].1, "s1");
    assert!(
        server.active_turns(Some("s2")).is_empty(),
        "another session does not leak in"
    );
    assert_eq!(server.active_turns(Some("s1")).len(), 1);

    server.finish_turn("t1");
    assert!(
        server.active_turns(None).is_empty(),
        "once it is done it must be gone"
    );
}

#[tokio::test]
async fn one_turn_at_a_time_per_session() {
    let server = server(Arc::new(Mock::new(vec![]))).await;

    let (_interrupt, _steering) = server
        .begin_turn("t1", "s1")
        .expect("the first one should get the session");
    let holder = server
        .begin_turn("t2", "s1")
        .expect_err("a second turn on the same session must be refused");
    assert_eq!(
        holder, "t1",
        "the refusal has to carry the occupant's turn id"
    );

    assert!(
        server.begin_turn("t3", "s2").is_ok(),
        "another session must not be dragged in"
    );

    server.finish_turn("t1");
    assert!(
        server.begin_turn("t4", "s1").is_ok(),
        "once the previous turn ends, it should pass"
    );
}

#[tokio::test]
async fn steering_needs_a_running_turn_and_keeps_arrival_order() {
    let server = server(Arc::new(Mock::new(vec![]))).await;

    assert!(
        server.steer("s1", "just one more thing").is_err(),
        "with no turn running it has to say so — silently starting a new turn would be a lie"
    );

    let (_interrupt, steering) = server.begin_turn("t1", "s1").expect("the session is idle");
    let (turn_id, queued) = server
        .steer("s1", "have a look at the README too")
        .expect("a turn is running");
    assert_eq!(turn_id, "t1");
    assert_eq!(queued, 1);
    let (_, queued) = server
        .steer("s1", "a second look at the CHANGELOG")
        .expect("a turn is running");
    assert_eq!(queued, 2);

    let mut channel = steering.lock().expect("lock");
    assert_eq!(
        channel.drain_pending(),
        vec![
            "have a look at the README too".to_string(),
            "a second look at the CHANGELOG".to_string()
        ],
        "handed to the model in arrival order"
    );
    assert!(
        channel.drain_pending().is_empty(),
        "what was taken is not repeated"
    );
    assert_eq!(
        channel.all().len(),
        2,
        "the transcript still shows everything"
    );
}

#[tokio::test]
async fn follow_up_needs_a_turn_in_flight_and_counts_the_line() {
    let server = server(Arc::new(Mock::new(vec![]))).await;

    assert!(
        matches!(
            server.follow_up("test", "I will do it later").await,
            crate::FollowUp::Idle
        ),
        "queueing while idle has no turn to wait on — the caller should just turn/start"
    );

    let (_interrupt, _steering) = server
        .begin_turn("t1", "test")
        .expect("the session is idle");
    assert!(matches!(
        server.follow_up("test", "the second thing").await,
        crate::FollowUp::Queued(1)
    ));
    assert!(matches!(
        server.follow_up("test", "the third thing").await,
        crate::FollowUp::Queued(2)
    ));
    assert_eq!(server.queue.count("test").await.expect("count"), 2);
    assert!(
        matches!(
            server.follow_up("other", "unrelated").await,
            crate::FollowUp::Idle
        ),
        "another session is not running, so this is unrelated"
    );
}

#[tokio::test]
async fn a_queued_message_becomes_its_own_turn_when_the_session_frees_up() {
    let mock = Mock::new(vec![
        Mock::text("the second thing is done."),
        Mock::text("the third thing is done."),
    ]);
    let server = server(Arc::new(mock)).await;

    let (_interrupt, _steering) = server
        .begin_turn("t1", "test")
        .expect("the session is idle");
    server.follow_up("test", "the second thing").await;
    server.follow_up("test", "the third thing").await;

    server.finish_turn("t1");
    crate::turn::drain_queue(&server, "test").await;

    let mut rows = Vec::new();
    for _ in 0..200 {
        rows = server
            .chat
            .messages("test", None, 50)
            .await
            .expect("read the conversation");
        let has_both = rows.iter().any(|row| row.content == "the second thing")
            && rows.iter().any(|row| row.content == "the third thing");
        if has_both && rows.len() >= 4 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let id_of = |text: &str| {
        rows.iter()
            .find(|row| row.content == text)
            .map(|row| row.id)
            .unwrap_or_else(|| panic!("a queued message never reached the conversation: {text}"))
    };
    assert!(
        id_of("the second thing") < id_of("the third thing"),
        "first in, first out"
    );
    assert_eq!(
        server.queue.count("test").await.expect("count"),
        0,
        "everything has run"
    );
    let mut released = false;
    for _ in 0..200 {
        if server.active_turns(Some("test")).is_empty() {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(released, "a finished turn has to give the session back");
}

#[tokio::test]
async fn a_finished_turn_lands_in_the_ledger() {
    let mock = Arc::new(Mock::new(vec![Mock::text("Four.")]));
    let server = server(mock).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let sink = EventSink::new(tx);
    let _ = crate::turn::run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "What is 2+2?".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await;
    while rx.try_recv().is_ok() {}

    let turns = server
        .ledger
        .turns("test", 10)
        .await
        .expect("list the ledger");
    assert_eq!(turns.len(), 1, "one turn is one turn");
    assert_eq!(
        turns[0].fidelity, "exact",
        "this turn was recorded live, not backfilled"
    );
    assert!(
        turns[0].preview.contains("2+2"),
        "the preview has to be recognisable"
    );

    let entries = server
        .ledger
        .entries("test", &turns[0].turn_id)
        .await
        .expect("read the ledger");
    assert!(
        entries.iter().any(|entry| entry.kind == "user"),
        "the request has to be accounted for"
    );
    assert!(
        entries.iter().any(|entry| entry.kind == "assistant"),
        "the reply has to be accounted for"
    );
    assert!(
        server
            .ledger
            .verify("test", &turns[0].turn_id)
            .await
            .expect("verify the ledger"),
        "written just now, so verification has to pass"
    );
}

#[test]
fn the_system_prompt_grounds_tool_results_in_current_turn_calls() {
    let prompt = crate::turn::build_system(crate::turn::PromptParts {
        soul: "A custom persona.",
        model: "test-model",
        provider: "mock",
        memory: "",
        memory_index: None,
        skill_index: None,
        skills: "",
        todo: None,
        summary: None,
        instructions: None,
        capabilities: None,
    })
    .full();
    let rule = prompt
        .lines()
        .find(|line| line.contains("lines record calls from earlier turns"))
        .expect("the system prompt must explain historical tool calls");
    assert!(rule.contains("a call in this turn returned it"), "{rule}");
    assert!(rule.contains("attribute earlier results"), "{rule}");
    assert!(
        rule.contains("failed without changing anything may be retried"),
        "{rule}"
    );
    assert!(
        rule.contains("partial effect is unknown, check it first"),
        "{rule}"
    );
    assert!(
        rule.contains("the work stays in this conversation"),
        "{rule}"
    );
}

#[test]
fn the_built_in_personas_only_forbid_redoing_calls_that_took_effect() {
    use super::prompt::{DEFAULT_SOUL, THIN_SOUL};
    for soul in [THIN_SOUL, DEFAULT_SOUL] {
        let line = soul
            .lines()
            .find(|line| line.contains("[tools used: ...]"))
            .expect("each persona explains the history footer");
        assert!(line.contains("已经生效的不要重做"), "{line}");
        assert!(line.contains("对方拒绝过的不要再申请"), "{line}");
    }
}

#[test]
fn the_system_prompt_carries_the_catalog_not_the_pages() {
    let catalog = "# Memory index\n\n2 pages · last updated 2026-11-12T00:00:00Z\n\n- [Alex](alex.md) · person — likes morning meetings\n";
    let with = crate::turn::build_system(crate::turn::PromptParts {
        soul: "You are Jeeves.",
        model: "test-model",
        provider: "mock",
        memory: "",
        memory_index: Some(catalog),
        skill_index: None,
        skills: "",
        todo: None,
        summary: None,
        instructions: None,
        capabilities: None,
    })
    .full();
    assert!(with.contains("Memory library"), "{with}");
    assert!(
        with.contains("[Alex](alex.md)"),
        "the catalog has to go in: {with}"
    );
    assert!(
        with.contains("read_page"),
        "the model has to be told to read pages on demand: {with}"
    );
    assert!(
        !with.contains("Alice likes"),
        "page bodies must not ride along: {with}"
    );

    let without = crate::turn::build_system(crate::turn::PromptParts {
        soul: "You are Jeeves.",
        model: "test-model",
        provider: "mock",
        memory: "",
        memory_index: Some(""),
        skill_index: None,
        skills: "",
        todo: None,
        summary: None,
        instructions: None,
        capabilities: None,
    })
    .full();
    assert!(
        !without.contains("Memory library"),
        "an empty index takes up no prompt: {without}"
    );
    assert!(
        without.len() < with.len(),
        "the catalog is what makes it longer"
    );
}

#[test]
fn the_thin_soul_trades_scaffolding_for_tokens_and_a_user_edit_wins() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let home = dir.path().to_path_buf();

    let thin = crate::turn::load_soul(&home, true);
    assert!(
        thin.contains("像一个懂行、了解对方的同事"),
        "the thin version has to keep the identity: {thin}"
    );
    assert!(
        !thin.contains("补充说明"),
        "the thin version must not carry the long scaffolding's details: {thin}"
    );

    let full = crate::turn::load_soul(&home, false);
    assert!(
        full.contains("补充说明"),
        "the full seed should carry the complete rules: {full}"
    );

    assert!(full.contains("复杂长任务"));
    assert!(thin.contains("复杂长任务"));
    for seed in [&thin, &full] {
        assert!(seed.contains("日常事务"));
        assert!(seed.contains("整理成要达成的结果"));
    }
    assert_eq!(crate::turn::load_soul(&home, true), thin);
    assert_eq!(std::fs::read_to_string(home.join("SOUL.md")).unwrap(), thin);

    std::fs::write(home.join("SOUL.md"), "Call me captain and nothing else.")
        .expect("write SOUL.md");
    assert_eq!(
        crate::turn::load_soul(&home, true),
        "Call me captain and nothing else."
    );
    assert_eq!(
        crate::turn::load_soul(&home, false),
        "Call me captain and nothing else."
    );
}

#[test]
fn both_previous_default_souls_upgrade_without_overwriting_edits() {
    use super::prompt::{DEFAULT_SOUL, THIN_SOUL};

    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("SOUL.md");
    let old_intro = "你是 Jeeves，负责完成复杂长任务的超级个人助理。把用户的目标、约束和交付物贯穿计划、执行和恢复；用可核验的结果说明完成情况。";
    for (thin, seed) in [(true, THIN_SOUL), (false, DEFAULT_SOUL)] {
        let (_, body) = seed.split_once("\n\n").unwrap();
        let old_body = body
            .replace(
                "动手前想清楚目标和完成的标准。做完后对照标准检查，再汇报。",
                "动手前想清楚目标和完成的标准；做完后对照标准检查，再汇报。\n- 遇到对方没说清楚、又会影响结果的地方，先问一个问题，不要猜。",
            )
            .replace(
                "[tools used: ...] 是你之前做过的调用。已经生效的不要重做；对方拒绝过的不要再申请，除非对方重新要求。",
                "[tools used: ...] 是你之前真正做过的事，不要重做。",
            );
        let retired = format!("{old_intro}\n\n{old_body}");
        assert_ne!(retired, seed);

        let edited = format!("{retired}\n\n称呼我为船长。");
        std::fs::write(&path, &edited).unwrap();
        assert_eq!(crate::turn::load_soul(home.path(), thin), edited);

        std::fs::write(&path, retired).unwrap();
        assert_eq!(crate::turn::load_soul(home.path(), thin), seed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), seed);
    }
}

#[test]
fn the_stable_prefix_does_not_move_between_turns() {
    let parts = |todo: Option<&'static str>| crate::turn::PromptParts {
        soul: "You are Jeeves.",
        model: "test-model",
        provider: "mock",
        memory: "Alex likes morning meetings",
        memory_index: Some("- [Alex](alex.md) — person"),
        skill_index: Some("- demo — a workflow"),
        skills: "",
        todo,
        summary: None,
        instructions: None,
        capabilities: None,
    };
    let first = crate::turn::build_system(parts(None));
    let second = crate::turn::build_system(parts(Some("## To-do\n- [ ] write tests")));

    assert_eq!(
        first.stable, second.stable,
        "the prefix must be byte-identical across turns"
    );
    assert_ne!(
        first.tail, second.tail,
        "the to-do board changed, so the tail should change"
    );
    assert!(
        first.tail.contains("Right now it is"),
        "the date lives in the tail"
    );
    let now = chrono::Local::now();
    assert!(first.tail.contains(&now.format("%Y-%m-%d").to_string()));
    assert!(!first.tail.contains(&now.format("%H:%M").to_string()));
}

#[test]
fn the_prompt_says_what_the_machine_cannot_do() {
    let off = jeeves_config::Settings::default();
    assert!(
        !off.exec_enabled && !off.write_enabled,
        "both switches are off by default"
    );

    let note =
        crate::turn::capability_note(&off, true).expect("with everything off it has to appear");
    assert!(note.contains("JEEVES_EXEC"), "{note}");
    assert!(note.contains("JEEVES_WRITE"), "{note}");
    assert!(
        note.contains("say so"),
        "the point is \"if you cannot, say so\": {note}"
    );
    assert!(
        note.contains("`glob`") && note.contains("`grep`"),
        "no shell is not no search: {note}"
    );

    let half = jeeves_config::Settings {
        exec_enabled: true,
        ..jeeves_config::Settings::default()
    };
    let note = crate::turn::capability_note(&half, true).expect("writing files is missing");
    assert!(
        note.contains("JEEVES_WRITE") && !note.contains("JEEVES_EXEC"),
        "{note}"
    );
    assert!(
        !note.contains("no shell") && !note.contains("Searching is not"),
        "with the shell on, the searching aside would only confuse: {note}"
    );

    let full = jeeves_config::Settings {
        exec_enabled: true,
        write_enabled: true,
        ..jeeves_config::Settings::default()
    };
    assert!(
        crate::turn::capability_note(&full, true).is_none(),
        "with both on they must take up no prompt"
    );
}

#[tokio::test]
async fn a_stop_does_not_wait_for_a_slow_preloop_step() {
    let cancel = jeeves_loop::Interrupt::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        trigger.cancel();
    });

    let started = std::time::Instant::now();
    let result = crate::turn::interruptible(
        Some(&cancel),
        "a long test step",
        tokio::time::sleep(std::time::Duration::from_secs(30)),
    )
    .await;

    assert!(
        result.is_none(),
        "an interrupted step should not return a result"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_millis(500),
        "a stop has to take effect at once, not after the step finishes: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn without_an_interrupt_the_step_just_runs() {
    let value = crate::turn::interruptible(None, "a test step", async { 7 }).await;
    assert_eq!(
        value,
        Some(7),
        "with no interrupt signal the behaviour is unchanged"
    );
}

fn outcome(name: &str, output: &str) -> jeeves_loop::ToolOutcome {
    jeeves_loop::ToolOutcome {
        success: true,
        tool_use_id: format!("tu-{name}"),
        name: name.to_string(),
        output: output.to_string(),
        duration_ms: 1,
    }
}

#[test]
fn a_folded_call_says_what_ran_how_much_and_the_first_line() {
    let folded = crate::turn::fold_tool_activity(
        "I have read it.",
        &[outcome("read_file", "first line\nsecond line\nthird line")],
        None,
    );
    assert!(folded.contains("read_file"), "{folded}");
    assert!(folded.contains("chars"), "{folded}");
    assert!(
        folded.contains("first line"),
        "the first line has to survive: {folded}"
    );
    assert!(
        !folded.contains("second line"),
        "only the first line is kept; the body comes back through read_file: {folded}"
    );
}

#[test]
fn a_spilled_result_keeps_the_address_to_read_it_again() {
    let output = "the first few lines\n…(90000 characters in total, truncated — 41 lines omitted; full output at spill/20261112/exec-101500-42.txt)\nthe last few lines";
    let folded =
        crate::turn::fold_tool_activity("done running.", &[outcome("run_command", output)], None);
    assert!(
        folded.contains("spill/20261112/exec-101500-42.txt"),
        "after folding, where to fetch it again has to remain: {folded}"
    );
    assert!(folded.contains("run_command"), "{folded}");
}

#[test]
fn a_result_without_spill_does_not_invent_an_address() {
    let folded = crate::turn::fold_tool_activity("all right.", &[outcome("echo", "hi")], None);
    assert!(
        !folded.contains("the full thing is at"),
        "the Chinese spill marker must be gone: {folded}"
    );
    assert!(folded.contains("hi"), "{folded}");
}

#[test]
fn a_medium_result_is_spilled_on_fold_so_later_turns_can_read_it_back() {
    let home = tempfile::tempdir().expect("temporary directory");
    let spill = home.path().join("spill");
    let body = format!(
        "fn main() {{\n{}}}\n",
        "    println!(\"a line the next turn needs\");\n".repeat(80)
    );
    assert!(body.chars().count() > crate::turn::history::FOLD_SPILL_CHARS);
    let folded =
        crate::turn::fold_tool_activity("read it.", &[outcome("read_file", &body)], Some(&spill));
    let address = folded
        .split("full output at ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .unwrap_or_else(|| panic!("the fold must carry an address: {folded}"));
    assert!(address.starts_with("spill/"), "relative to home: {address}");
    let kept = std::fs::read_to_string(home.path().join(address)).expect("spilled file");
    assert_eq!(kept, body, "the whole body is kept, byte for byte");
    assert!(folded.contains("first line: fn main() {"), "{folded}");
}

#[test]
fn a_short_result_is_not_spilled_on_fold() {
    let home = tempfile::tempdir().expect("temporary directory");
    let spill = home.path().join("spill");
    let folded = crate::turn::fold_tool_activity("ok.", &[outcome("echo", "hi")], Some(&spill));
    assert!(!folded.contains("full output at"), "{folded}");
    assert!(
        !spill.exists(),
        "no file for a result the model can cheaply re-run"
    );
}

#[test]
fn an_already_spilled_result_is_not_spilled_twice() {
    let home = tempfile::tempdir().expect("temporary directory");
    let spill = home.path().join("spill");
    let output = format!(
        "{}\n…(truncated; full output at spill/20261112/exec-101500-42.txt)",
        "x".repeat(3_000)
    );
    let folded =
        crate::turn::fold_tool_activity("done.", &[outcome("run_command", &output)], Some(&spill));
    assert!(
        folded.contains("spill/20261112/exec-101500-42.txt"),
        "the original address is kept: {folded}"
    );
    assert!(
        !spill.exists(),
        "no second copy of an output that already has a home"
    );
}

#[test]
fn without_a_spill_dir_a_medium_result_folds_as_before() {
    let body = "y".repeat(3_000);
    let folded = crate::turn::fold_tool_activity("ok.", &[outcome("read_file", &body)], None);
    assert!(!folded.contains("full output at"), "{folded}");
}

#[test]
fn no_tool_calls_leaves_the_reply_untouched() {
    assert_eq!(
        crate::turn::fold_tool_activity("just say one line", &[], None),
        "just say one line",
        "nothing to fold leaves it alone"
    );
}

#[tokio::test]
async fn interrupted_queue_claim_is_visible_and_is_not_replayed() {
    let mock = Mock::new(Vec::new());
    let server = server(Arc::new(mock)).await;
    server
        .queue
        .push("test", "uncertain instruction")
        .await
        .unwrap();
    let claim = server.queue.pop("test").await.unwrap().unwrap();
    server.restore_tasks().await;
    let rows = server.chat.messages("test", None, 50).await.unwrap();
    assert!(rows.iter().any(|row| row.content == claim.message));
    assert!(
        rows.iter()
            .any(|row| row.content.contains("outcome is unknown"))
    );
    assert!(server.queue.pop("test").await.unwrap().is_none());
    assert!(
        server
            .queue
            .reconcile_interrupted()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(server.active_turns(Some("test")).is_empty());
}

#[tokio::test]
async fn the_turn_cap_refuses_at_the_limit_and_frees_on_finish() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    assert!(server.turn_capacity().is_ok(), "no cap by default");
    let mut settings = server.settings();
    settings.max_concurrent_turns = 1;
    server.set_settings(settings);
    assert!(server.turn_capacity().is_ok(), "nothing is running yet");
    server
        .begin_turn("t_cap", "a")
        .expect("the session is idle");
    assert!(server.turn_capacity().is_err(), "the cap is reached");
    server.finish_turn("t_cap");
    assert!(
        server.turn_capacity().is_ok(),
        "finishing a turn frees the slot"
    );
}

#[tokio::test]
async fn the_home_cap_refuses_when_the_home_is_full() {
    let server = server(Arc::new(Mock::new(vec![]))).await;
    let mut settings = server.settings();
    let home = settings.home.clone();
    std::fs::write(home.join("big.bin"), vec![0u8; 2 * 1024 * 1024]).expect("write filler");
    settings.home_max_mb = 1;
    server.set_settings(settings.clone());
    assert!(
        server.home_capacity().await.is_err(),
        "2 MB used, 1 MB allowed"
    );
    settings.home_max_mb = 0;
    server.set_settings(settings);
    assert!(server.home_capacity().await.is_ok(), "0 means no cap");
}

#[test]
fn a_footer_the_model_copied_from_history_is_dropped_before_folding() {
    let reply = "The edit was refused.\n[tools used: edit(40 chars: Error: Action denied or approval expired)]\n";
    let stripped = crate::turn::strip_echoed_footer(reply);
    assert_eq!(stripped, "The edit was refused.");
    let folded = crate::turn::fold_tool_activity(
        stripped,
        &[outcome(
            "edit",
            "Error: edit failed: notes.txt changed on disk",
        )],
        None,
    );
    assert_eq!(folded.matches("[tools used:").count(), 1, "{folded}");
    assert!(folded.contains("changed on disk"), "{folded}");
    assert!(!folded.contains("approval expired"), "{folded}");
}

#[test]
fn only_trailing_footer_lines_are_dropped() {
    assert_eq!(
        crate::turn::strip_echoed_footer(
            "a\n[tools used: x(1 chars: y)]\n[tools used: z(1 chars: w)]"
        ),
        "a"
    );
    let quoted = "The footer looks like `[tools used: …]`.\nThat is all.";
    assert_eq!(crate::turn::strip_echoed_footer(quoted), quoted);
    assert_eq!(
        crate::turn::strip_echoed_footer("[tools used: x(1 chars: y)]"),
        ""
    );
}

#[tokio::test]
async fn an_unnamed_session_is_titled_once_after_its_first_exchange() {
    let mock = Arc::new(Mock::with_outcomes(vec![Ok(Mock::text(
        "{\"title\": \"Postgres pool exhaustion\"}",
    ))]));
    let server = server(mock.clone()).await;
    server
        .chat
        .append_exchange(
            "my postgres pool keeps running out, log below ...",
            "raise max_connections",
            "test",
            "cli",
            None,
        )
        .await
        .expect("write the session");
    let resolved = server.resolved().expect("a model is configured");
    crate::turn::title::name_session(&server, &resolved, "test").await;
    let row = server.chat.session("test").await.unwrap().unwrap();
    assert_eq!(row.title, "Postgres pool exhaustion");
    let request = mock.received.lock().unwrap()[0].clone();
    assert_eq!(request.model, "test-small", "titles use the small model");
    assert!(request.tools.is_empty(), "the titler has no tools");

    crate::turn::title::name_session(&server, &resolved, "test").await;
    assert_eq!(
        mock.received.lock().unwrap().len(),
        1,
        "a titled session is not asked again"
    );
}

#[test]
fn the_live_ask_carries_the_minute_and_offset_it_was_sent() {
    use chrono::TimeZone;
    let at = chrono::Local
        .with_ymd_and_hms(2026, 11, 24, 15, 7, 0)
        .single()
        .expect("an unambiguous local time");
    let stamp = crate::turn::prompt::sent_stamp(at);
    assert!(
        stamp.starts_with("[Sent 2026-11-24 15:07 (Tuesday), UTC"),
        "{stamp}"
    );
    assert!(stamp.ends_with(']'), "{stamp}");
}
