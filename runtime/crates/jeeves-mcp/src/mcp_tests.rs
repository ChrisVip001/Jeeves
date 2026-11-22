//! Tests for the MCP client.
//!
//! Three layers:
//!   1. Config decisions and renaming — pure functions, no process, no network.
//!   2. The two wire framings — in-memory pipes (stdio) and a local fake HTTP server.
//!   3. A real subprocess server (a fake MCP written in `python3`, no SDK),
//!      end-to-end up to registering into ToolRegistry and calling successfully.
//!
//! Layer 3 needs python3; skip if the machine lacks it (never fake a pass); `scripts/smoke.sh`
//! also carries an end-to-end of the same flavor.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::config::{self, Config, ServerSpec, Transport as TransportSpec};
use crate::transport::{HttpSession, StdioSession};
use crate::McpClient;

fn no_env(_key: &str) -> Option<String> {
    None
}

#[test]
fn model_safe_name_keeps_what_the_provider_accepts() {
    assert_eq!(
        config::model_safe_name("notes", "remember"),
        "notes_remember"
    );
    assert_eq!(
        config::model_safe_name("team-memory", "memory.remember"),
        "team-memory_memory_remember"
    );
    let long = config::model_safe_name("s", &"x".repeat(200));
    assert_eq!(long.len(), 64);
    assert!(
        !long.starts_with('_'),
        "a truncated name must not start with _: {long}"
    );
}

#[test]
fn config_refuses_two_transports_and_two_credentials() {
    let both_transports = ServerSpec {
        name: "x".to_string(),
        url: Some("https://h/mcp".to_string()),
        command: Some("npx".to_string()),
        ..ServerSpec::default()
    };
    assert!(config::resolve(&both_transports, &no_env)
        .unwrap_err()
        .contains("pick one transport"));

    let both_creds = ServerSpec {
        name: "x".to_string(),
        url: Some("https://h/mcp".to_string()),
        auth_env: Some("K".to_string()),
        oauth: Some(true),
        ..ServerSpec::default()
    };
    assert!(config::resolve(&both_creds, &no_env)
        .unwrap_err()
        .contains("pick one"));

    let neither = ServerSpec {
        name: "x".to_string(),
        ..ServerSpec::default()
    };
    assert!(config::resolve(&neither, &no_env).is_err());
}

#[test]
fn oauth_resolves_to_anonymous_http_the_token_layer_sits_above() {
    let spec = ServerSpec {
        name: "x".to_string(),
        url: Some("https://h/mcp".to_string()),
        oauth: Some(true),
        ..ServerSpec::default()
    };
    match config::resolve(&spec, &no_env).expect("oauth is a legal config") {
        config::Transport::Http { token, .. } => assert!(token.is_none()),
        other => panic!("oauth should take the Http transport: {other:?}"),
    }
}

#[test]
fn a_named_but_unset_variable_is_refused_before_connecting() {
    let spec = ServerSpec {
        name: "x".to_string(),
        url: Some("https://h/mcp".to_string()),
        auth_env: Some("NOTES_API_KEY".to_string()),
        ..ServerSpec::default()
    };
    let message = config::resolve(&spec, &no_env).unwrap_err();
    assert!(message.contains("NOTES_API_KEY"), "{message}");

    let found = config::resolve(&spec, &|key| {
        (key == "NOTES_API_KEY").then(|| "s3cret".to_string())
    })
    .expect("with a value it connects");
    assert_eq!(
        found,
        TransportSpec::Http {
            url: "https://h/mcp".to_string(),
            token: Some("s3cret".to_string()),
        }
    );
}

#[test]
fn stdio_and_http_are_chosen_by_shape() {
    let stdio = ServerSpec {
        name: "fs".to_string(),
        command: Some("npx".to_string()),
        args: Some(vec!["-y".to_string(), "server".to_string()]),
        env: Some(BTreeMap::from([("A".to_string(), "1".to_string())])),
        ..ServerSpec::default()
    };
    assert_eq!(
        config::resolve(&stdio, &no_env).expect("stdio"),
        TransportSpec::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "server".to_string()],
            env: Some(BTreeMap::from([("A".to_string(), "1".to_string())])),
        }
    );

    let anonymous = ServerSpec {
        name: "pub".to_string(),
        url: Some("https://h/mcp".to_string()),
        ..ServerSpec::default()
    };
    assert_eq!(
        config::resolve(&anonymous, &no_env).expect("http"),
        TransportSpec::Http {
            url: "https://h/mcp".to_string(),
            token: None,
        }
    );
}

#[test]
fn the_auth_hint_names_what_the_caller_can_check() {
    let anonymous = TransportSpec::Http {
        url: "https://h/mcp".to_string(),
        token: None,
    };
    let hint = config::auth_hint(&anonymous).expect("it should offer a hint");
    assert!(hint.contains("https://h/mcp"));
    assert!(hint.contains("auth_env"));
    let stdio = TransportSpec::Stdio {
        command: "npx".to_string(),
        args: vec![],
        env: None,
    };
    assert!(config::auth_hint(&stdio).is_none());
}

#[test]
fn config_parses_the_documented_shape() {
    let config = Config::parse(
        r#"{"servers": [{"name": "fs", "command": "npx", "args": ["-y", "s"]},
                        {"name": "notes", "url": "https://h/mcp", "auth_env": "K"}]}"#,
    )
    .expect("it resolves");
    assert_eq!(config.servers.len(), 2);
    assert_eq!(config.servers[0].command.as_deref(), Some("npx"));
    assert_eq!(config.servers[1].auth_env.as_deref(), Some("K"));
    assert!(Config::parse("not json").is_err());
}

#[tokio::test]
async fn no_config_file_means_no_mcp() {
    let client = McpClient::connect(std::path::Path::new("/nope/mcp.json")).await;
    assert!(client.servers().is_empty());
    assert!(
        client.warnings().is_empty(),
        "unconfigured is not a mistake"
    );
    assert!(client.tools().is_empty());
}

#[tokio::test]
async fn a_broken_config_file_says_so() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("mcp.json");
    std::fs::write(&path, "{ not json").expect("write");
    let client = McpClient::connect(&path).await;
    assert!(client.servers().is_empty());
    assert_eq!(client.warnings().len(), 1);
    assert!(client.warnings()[0].contains("mcp.json"));
}

#[test]
fn config_paths_put_the_users_file_first() {
    let home = std::path::Path::new("/home/u/.jeeves");
    assert_eq!(
        config::config_paths_for(home, None),
        vec![std::path::PathBuf::from("/home/u/.jeeves/mcp.json")],
        "with no extra directory, only the user's own is read"
    );

    let extra = std::env::join_paths(["/etc/jeeves/mcp.json", "/opt/pack/mcp.json"])
        .expect("the paths join");
    assert_eq!(
        config::config_paths_for(home, Some(&extra)),
        vec![
            std::path::PathBuf::from("/home/u/.jeeves/mcp.json"),
            std::path::PathBuf::from("/etc/jeeves/mcp.json"),
            std::path::PathBuf::from("/opt/pack/mcp.json"),
        ],
        "the user's comes first — read first, so it wins on a name clash"
    );
}

#[tokio::test]
async fn the_users_config_file_wins_over_the_extras() {
    let fake = fake_http(vec![
        Reply {
            status: 200,
            content_type: "application/json",
            body: r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#
                .to_string(),
        },
        Reply {
            status: 202,
            content_type: "application/json",
            body: String::new(),
        },
        Reply {
            status: 200,
            content_type: "application/json",
            body: r#"{"jsonrpc":"2.0","id":@id,"result":{"tools":[{"name":"reverse"}]}}"#
                .to_string(),
        },
    ])
    .await;

    let dir = tempfile::tempdir().expect("temp dir");
    let user = dir.path().join("mcp.json");
    std::fs::write(
        &user,
        json!({"servers": [{"name": "demo", "url": fake.url}]}).to_string(),
    )
    .expect("write the user config");
    let extra = dir.path().join("pack.json");
    std::fs::write(
        &extra,
        json!({"servers": [
            {"name": "demo", "command": "definitely-not-a-command-xyz"},
            {"name": "other", "command": "definitely-not-a-command-xyz"}
        ]})
        .to_string(),
    )
    .expect("write the extra config");

    let client = McpClient::connect_paths(&[user, extra]).await;
    assert_eq!(
        client.servers(),
        vec!["demo"],
        "the local one survives; the duplicate in the extra file is skipped and does not displace it"
    );
    assert_eq!(
        client.warnings().len(),
        1,
        "only the unshadowed offline server is started: {:?}",
        client.warnings()
    );
}

#[tokio::test]
async fn invalid_owner_manifest_cannot_fall_through_to_a_pack() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("mcp.json");
    let pack = dir.path().join("pack.json");
    std::fs::write(
        &user,
        r#"{"servers":[{"name":"tasks","command":"unused","manifest":"connector.json"}]}"#,
    )
    .unwrap();
    std::fs::write(
        &pack,
        r#"{"servers":[{"name":"tasks","command":"should-not-start"}]}"#,
    )
    .unwrap();
    let client = McpClient::connect_paths(&[user, pack]).await;
    assert!(client.servers().is_empty());
    assert_eq!(client.warnings().len(), 1);
    assert!(client.warnings()[0].contains("connector.json"));
}

#[tokio::test]
async fn stdio_session_frames_json_lines_and_skips_foreign_messages() {
    let (client_side, server_side) = tokio::io::duplex(8192);
    let (server_read, mut server_write) = tokio::io::split(server_side);
    let server = tokio::spawn(async move {
        let mut lines = BufReader::new(server_read).lines();
        while let Some(line) = lines.next_line().await.expect("read") {
            let message: serde_json::Value = serde_json::from_str(&line).expect("JSON");
            let method = message["method"].as_str().unwrap_or("").to_string();
            let Some(id) = message.get("id").cloned() else {
                continue;
            };
            let result = match method.as_str() {
                "initialize" => json!({"protocolVersion": "2025-06-18"}),
                "tools/list" => {
                    server_write
                        .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n")
                        .await
                        .expect("write");
                    json!({"tools": [{"name": "reverse"}]})
                }
                _ => json!({}),
            };
            let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
            server_write
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .expect("write");
        }
    });

    let (client_read, client_write) = tokio::io::split(client_side);
    let mut session = StdioSession::new(client_read, client_write);
    session.initialize().await.expect("handshake");
    let tools = session.list_tools().await.expect("list the tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "reverse");
    assert_eq!(
        tools[0].schema(),
        json!({"type": "object", "properties": {}})
    );
    server.abort();
}

#[tokio::test]
async fn stdio_session_reports_a_server_that_goes_away() {
    let (client_side, server_side) = tokio::io::duplex(1024);
    drop(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);
    let mut session = StdioSession::new(client_read, client_write);
    let error = session.initialize().await.unwrap_err();
    assert!(error.contains("could not write"), "{error}");

    let (client_side, server_side) = tokio::io::duplex(1024);
    let (mut server_read, server_write) = tokio::io::split(server_side);
    drop(server_write);
    let drain = tokio::spawn(async move {
        let mut line = String::new();
        let _ = BufReader::new(&mut server_read).read_line(&mut line).await;
    });
    let (client_read, client_write) = tokio::io::split(client_side);
    let mut session = StdioSession::new(client_read, client_write);
    let error = session.initialize().await.unwrap_err();
    assert!(
        error.contains("the server closed the connection"),
        "{error}"
    );
    drain.await.expect("drain that line");
}

struct FakeHttp {
    url: String,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

async fn fake_http(script: Vec<Reply>) -> FakeHttp {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("the address"));
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let index = Arc::new(AtomicUsize::new(0));
    let (script, sink) = (Arc::new(script), seen.clone());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (script, index, sink) = (script.clone(), index.clone(), sink.clone());
            tokio::spawn(async move {
                loop {
                    let Some((headers, body)) = read_http_request(&mut stream).await else {
                        return;
                    };
                    sink.lock().await.push((headers, body.clone()));
                    let at = index.fetch_add(1, Ordering::SeqCst);
                    let reply = &script[at.min(script.len() - 1)];
                    let id = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|message| message.get("id").cloned())
                        .unwrap_or(Value::Null);
                    let payload = reply.body.replace("@id", &id.to_string());
                    let head = format!(
                        "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nmcp-session-id: abc\r\n\r\n",
                        reply.status,
                        reply.content_type,
                        payload.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(payload.as_bytes()).await;
                    let _ = stream.flush().await;
                }
            });
        }
    });
    FakeHttp { url, seen }
}

#[tokio::test]
async fn structured_only_search_results_are_not_lost() {
    let fake = fake_http(vec![Reply {
        status: 200,
        content_type: "application/json",
        body: r#"{"jsonrpc":"2.0","id":@id,"result":{"structuredContent":{"results":[{"url":"https://example.com/"}]}}}"#.into(),
    }]).await;
    let text = HttpSession::new(&fake.url, None)
        .call_tool("web_search", json!({}))
        .await
        .unwrap();
    let result: Value = serde_json::from_str(&text.text).expect("preserve structured content");
    assert_eq!(result["results"][0]["url"], "https://example.com/");
}

#[tokio::test]
async fn explicit_oauth_challenge_is_reported_as_missing_scope() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await.unwrap();
        stream.write_all(b"HTTP/1.1 403 Forbidden\r\nWWW-Authenticate: Bearer error=\"insufficient_scope\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.json");
    std::fs::write(
        &path,
        json!({"servers":[{"name":"scope","url":url}]}).to_string(),
    )
    .unwrap();
    let client = crate::McpClient::connect(&path).await;
    assert_eq!(client.missing_scope(), &["scope"]);
}

#[tokio::test]
async fn oversized_http_reply_is_refused_before_parsing() {
    let fake = fake_http(vec![Reply {
        status: 200,
        content_type: "application/json",
        body: "x".repeat(2 * 1024 * 1024 + 1),
    }])
    .await;
    let error = HttpSession::new(&fake.url, None)
        .request("tools/list", json!({}))
        .await
        .unwrap_err();
    assert!(error.contains("2 MiB"), "{error}");
}

async fn read_http_request<S>(stream: &mut S) -> Option<(String, String)>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
        .await
        .ok()?;
    Some((headers, String::from_utf8_lossy(&body).to_string()))
}

#[tokio::test]
async fn http_session_handshakes_sends_the_token_and_reads_sse() {
    let reply = |body: &str| Reply {
        status: 200,
        content_type: "application/json",
        body: body.to_string(),
    };
    let fake = fake_http(vec![
        reply(r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#),
        Reply {
            status: 202,
            content_type: "application/json",
            body: String::new(),
        },
        Reply {
            status: 200,
            content_type: "text/event-stream",
            body: "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/x\"}\n\n\
                   event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":@id,\"result\":{\"tools\":[{\"name\":\"reverse\"}]}}\n\n"
                .to_string(),
        },
        reply(r#"{"jsonrpc":"2.0","id":@id,"result":{"content":[{"type":"text","text":"cba"}]}}"#),
    ])
    .await;

    let mut session = HttpSession::new(&fake.url, Some("s3cret".to_string()));
    session.initialize().await.expect("handshake");
    let tools = session
        .list_tools()
        .await
        .expect("list the tools (over SSE)");
    assert_eq!(tools[0].name, "reverse");
    let text = session
        .call_tool("reverse", json!({"text": "abc"}))
        .await
        .expect("call it");
    assert_eq!(text.text, "cba");

    let seen = fake.seen.lock().await;
    assert!(seen[0].0.contains("Bearer s3cret"), "{}", seen[0].0);
    assert!(seen[1].0.contains("mcp-session-id: abc"), "{}", seen[1].0);
    assert!(seen[2].0.contains("mcp-session-id: abc"), "{}", seen[2].0);
    assert!(
        seen[1].1.contains("notifications/initialized"),
        "{}",
        seen[1].1
    );
    assert!(seen[2].0.contains("text/event-stream"), "{}", seen[2].0);
}

#[tokio::test]
async fn http_failure_reports_the_status_code() {
    let fake = fake_http(vec![Reply {
        status: 401,
        content_type: "text/html",
        body: "<html><body>unauthorized</body></html>".to_string(),
    }])
    .await;
    let mut session = HttpSession::new(&fake.url, None);
    let error = session.initialize().await.unwrap_err();
    assert!(error.contains("401"), "{error}");
    assert!(error.len() < 200, "{error}");
}

#[tokio::test]
async fn a_dead_transport_is_rebuilt_without_replaying_an_unknown_outcome() {
    let reply = |body: &str| Reply {
        status: 200,
        content_type: "application/json",
        body: body.to_string(),
    };
    let fake = fake_http(vec![
        Reply {
            status: 500,
            content_type: "text/plain",
            body: "connection lost after commit".into(),
        },
        reply(r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#),
        Reply {
            status: 202,
            content_type: "application/json",
            body: String::new(),
        },
        reply(
            r#"{"jsonrpc":"2.0","id":@id,"result":{"content":[{"type":"text","text":"alive"}]}}"#,
        ),
    ])
    .await;

    let dead = crate::transport::Transport::Http(HttpSession::new(&fake.url, None));
    let connection = crate::Connection::with_policy("x", dead, crate::BreakerPolicy::default())
        .with_spec(TransportSpec::Http {
            url: fake.url.clone(),
            token: None,
        });

    let text = connection.call("echo", json!({})).await;
    assert!(text.contains("outcome unknown"), "{text}");
    assert_eq!(
        fake.seen.lock().await.len(),
        3,
        "one tool call plus the reconnect handshake; no replay"
    );
    let next = connection.call("echo", json!({})).await;
    assert_eq!(
        next, "alive",
        "an explicit later call can use the rebuilt connection"
    );
    assert_eq!(fake.seen.lock().await.len(), 4);
}

#[tokio::test]
async fn a_transport_without_a_spec_says_it_cannot_be_rebuilt() {
    let dead = crate::transport::Transport::Http(HttpSession::new("http://127.0.0.1:1/mcp", None));
    let connection = crate::Connection::with_policy("x", dead, crate::BreakerPolicy::default());
    let text = connection.call("echo", json!({})).await;
    assert!(text.contains("cannot be rebuilt"), "{text}");
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
    method = msg.get("method")
    if "id" not in msg:
        continue
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "fake", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [
            {"name": "reverse", "description": "reverse the text",
             "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}},
                             "required": ["text"]}},
            {"name": "dotted.name"},
            {"name": "boom"}]}
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name")
        if name == "reverse":
            text = (params.get("arguments") or {}).get("text", "")
            result = {"content": [{"type": "text", "text": text[::-1]}]}
        elif name == "boom":
            result = {"content": [{"type": "text", "text": "blew up"}], "isError": True}
        else:
            result = {"content": [{"type": "text", "text": "ok"}]}
    else:
        send({"jsonrpc": "2.0", "id": msg["id"],
              "error": {"code": -32601, "message": "no such method"}})
        continue
    send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
"#;

fn python3() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("python3")
        .arg("--version")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| std::path::PathBuf::from("python3"))
}

async fn tool_ctx() -> jeeves_tools::ToolCtx {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    jeeves_tools::ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        activate: None,
        cwd: None,
        approval: None,
        jobs: None,
        turn_limits: std::sync::Arc::new(jeeves_tools::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home: std::path::PathBuf::from(".jeeves"),
        on_delta: None,
    }
}

#[tokio::test]
async fn a_real_subprocess_server_registers_working_tools() {
    let Some(python) = python3() else {
        eprintln!("skipped: no python3 on this machine");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("fake_mcp_server.py");
    std::fs::write(&script, FAKE_SERVER).expect("write the server");
    let config = dir.path().join("mcp.json");
    std::fs::write(
        &config,
        json!({"servers": [{"name": "demo", "command": python.display().to_string(),
        "args": [script.display().to_string()],
        "manifest":{"category":"other","tools":{
            "reverse":{"effect":"read"}, "dotted.name":{"effect":"read"}, "boom":{"effect":"read"}
        }}}]})
        .to_string(),
    )
    .expect("write the config");

    let client = McpClient::connect(&config).await;
    assert_eq!(
        client.warnings(),
        Vec::<String>::new(),
        "there should be no warning"
    );
    assert_eq!(client.servers(), vec!["demo"]);

    let mut registry = jeeves_tools::ToolRegistry::new();
    client.register_tools(&mut registry);
    assert_eq!(
        registry.names(),
        vec!["demo_boom", "demo_dotted_name", "demo_reverse"]
    );

    let reverse = registry.get("demo_reverse").expect("reverse is there");
    assert_eq!(reverse.description, "[MCP:demo] reverse the text");
    assert_eq!(reverse.input_schema["required"], json!(["text"]));
    assert_eq!(
        registry
            .get("demo_dotted_name")
            .expect("present")
            .input_schema,
        json!({"type": "object", "properties": {}})
    );

    let ctx = tool_ctx().await;
    assert_eq!(
        registry
            .execute(ctx.clone(), "demo_reverse", json!({"text": "abc"}))
            .await,
        "cba"
    );
    assert_eq!(
        registry
            .execute(ctx.clone(), "demo_dotted_name", json!({}))
            .await,
        "ok"
    );
    let outcome = registry.execute_result(ctx, "demo_boom", json!({})).await;
    assert!(!outcome.success);
    let failed = outcome.text;
    assert!(failed.contains("MCP tool boom failed:"), "{failed}");
    assert!(
        !failed.contains("outcome unknown"),
        "a remote tool error is a known outcome: {failed}"
    );
    assert!(failed.contains("blew up"), "{failed}");
}

#[tokio::test]
async fn a_server_that_cannot_start_is_skipped_with_a_warning() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = dir.path().join("mcp.json");
    std::fs::write(
        &config,
        json!({"servers": [{"name": "ghost", "command": "definitely-not-a-command-xyz"}]})
            .to_string(),
    )
    .expect("write the config");

    let client = McpClient::connect(&config).await;
    assert!(client.servers().is_empty());
    assert_eq!(client.warnings().len(), 1, "{:?}", client.warnings());
    assert!(
        client.warnings()[0].contains("ghost"),
        "{:?}",
        client.warnings()
    );
    assert_eq!(
        client.call("ghost", "reverse", json!({})).await,
        "Error: MCP server 'ghost' is not connected."
    );
}

#[tokio::test]
async fn a_failing_server_gets_circuit_broken() {
    use crate::{BreakerPolicy, Connection, Transport};
    use std::time::Duration;

    let fake = fake_http(vec![
        Reply {
            status: 200,
            content_type: "application/json",
            body: r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#
                .to_string(),
        },
        Reply {
            status: 202,
            content_type: "application/json",
            body: String::new(),
        },
        Reply {
            status: 500,
            content_type: "application/json",
            body: "boom".to_string(),
        },
    ])
    .await;

    let mut session = HttpSession::new(&fake.url, None);
    session.initialize().await.expect("handshake");

    let connection = Connection::with_policy(
        "flaky",
        Transport::Http(session),
        BreakerPolicy {
            failures: 2,
            cooldown: Duration::from_millis(150),
        },
    );

    let first = connection.call("do", json!({})).await;
    let second = connection.call("do", json!({})).await;
    assert!(first.contains("failed"), "{first}");
    assert!(second.contains("failed"), "{second}");

    let before = fake.seen.lock().await.len();
    let skipped = connection.call("do", json!({})).await;
    assert!(skipped.contains("circuit breaker"), "{skipped}");
    assert!(skipped.contains("was skipped"), "{skipped}");
    assert_eq!(
        fake.seen.lock().await.len(),
        before,
        "nothing may go out while the breaker is open"
    );

    tokio::time::sleep(Duration::from_millis(220)).await;
    let probe = connection.call("do", json!({})).await;
    assert!(
        !probe.contains("circuit breaker"),
        "once cooled down one probe has to be let through; got: {probe}"
    );
    assert!(
        probe.contains("failed"),
        "the probe really did knock: {probe}"
    );
}

#[test]
fn two_servers_that_collide_get_distinct_names() {
    use std::collections::HashSet;

    let mut taken: HashSet<String> = HashSet::new();
    let (first, renamed) = config::model_safe_name_unique("a", "b_c", &mut taken);
    assert_eq!(first, "a_b_c");
    assert!(!renamed);

    let (second, renamed) = config::model_safe_name_unique("a_b", "c", &mut taken);
    assert!(renamed, "a collision has to be renamed");
    assert_ne!(second, first, "the earlier one must not be overwritten");
    assert!(second.starts_with("a_b_c_"), "{second}");
    assert!(
        second.len() <= 64,
        "the 64-character cap still applies: {second}"
    );
}

#[test]
fn a_renamed_long_tool_name_still_fits() {
    use std::collections::HashSet;

    let mut taken: HashSet<String> = HashSet::new();
    let long = "x".repeat(200);
    let (first, _) = config::model_safe_name_unique("s", &long, &mut taken);
    let (second, renamed) = config::model_safe_name_unique("s", &long, &mut taken);
    assert!(renamed);
    assert!(second.len() <= 64, "{second}");
    assert_ne!(first, second);
}

fn json_reply(body: &str) -> Reply {
    Reply {
        status: 200,
        content_type: "application/json",
        body: body.to_string(),
    }
}

fn accepted() -> Reply {
    Reply {
        status: 202,
        content_type: "application/json",
        body: String::new(),
    }
}

#[tokio::test]
async fn http_session_restarts_after_a_404_and_resends_the_request() {
    let init = r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#;
    let fake = fake_http(vec![
        json_reply(init),
        accepted(),
        Reply {
            status: 404,
            content_type: "text/plain",
            body: "unknown session".into(),
        },
        json_reply(init),
        accepted(),
        json_reply(
            r#"{"jsonrpc":"2.0","id":@id,"result":{"content":[{"type":"text","text":"back"}]}}"#,
        ),
    ])
    .await;

    let mut session = HttpSession::new(&fake.url, None);
    session.initialize().await.expect("handshake");
    let text = session
        .call_tool("echo", json!({}))
        .await
        .expect("the retried call succeeds");
    assert_eq!(text.text, "back");

    let seen = fake.seen.lock().await;
    assert_eq!(seen.len(), 6, "{seen:?}");
    assert!(seen[2].1.contains("tools/call"), "{}", seen[2].1);
    assert!(seen[3].1.contains("\"initialize\""), "{}", seen[3].1);
    assert!(
        !seen[3].0.to_ascii_lowercase().contains("mcp-session-id"),
        "the new handshake must not carry the dead session id: {}",
        seen[3].0
    );
    assert!(seen[5].1.contains("tools/call"), "{}", seen[5].1);
}

#[tokio::test]
async fn http_session_does_not_restart_on_a_404_without_a_session() {
    let fake = fake_http(vec![Reply {
        status: 404,
        content_type: "text/plain",
        body: "no such endpoint".into(),
    }])
    .await;
    let error = HttpSession::new(&fake.url, None)
        .initialize()
        .await
        .unwrap_err();
    assert!(error.contains("HTTP 404"), "{error}");
    assert_eq!(fake.seen.lock().await.len(), 1);
}

#[tokio::test]
async fn http_session_close_sends_delete_with_the_session_id() {
    let fake = fake_http(vec![
        json_reply(r#"{"jsonrpc":"2.0","id":@id,"result":{"protocolVersion":"2025-06-18"}}"#),
        accepted(),
        Reply {
            status: 204,
            content_type: "text/plain",
            body: String::new(),
        },
    ])
    .await;
    let mut session = HttpSession::new(&fake.url, Some("s3cret".to_string()));
    session.initialize().await.expect("handshake");
    session.close().await;
    session.close().await;

    let seen = fake.seen.lock().await;
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen[2].0.starts_with("DELETE "), "{}", seen[2].0);
    assert!(seen[2].0.contains("mcp-session-id: abc"), "{}", seen[2].0);
    assert!(seen[2].0.contains("Bearer s3cret"), "{}", seen[2].0);
}

#[tokio::test]
async fn a_malformed_list_is_an_error_and_an_empty_list_is_authoritative() {
    let (client, peer) = tokio::io::duplex(4096);
    let (r, w) = tokio::io::split(client);
    let (pr, mut pw) = tokio::io::split(peer);
    let peer = tokio::spawn(async move {
        let mut lines = BufReader::new(pr).lines();
        for result in [json!({}), json!({"tools":[]})] {
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
            pw.write_all(format!("{response}\n").as_bytes())
                .await
                .unwrap();
        }
    });
    let mut session = StdioSession::new(r, w);
    assert!(session.list_tools().await.is_err());
    assert!(session.list_tools().await.unwrap().is_empty());
    peer.await.unwrap();
}

#[tokio::test]
async fn an_open_oauth_transport_uses_rotation_and_stops_on_revocation_or_address_change() {
    use crate::oauth::{StoredAuth, StoredTokens, TokenStore};
    let result = r#"{"jsonrpc":"2.0","id":@id,"result":{"tools":[]}}"#;
    let peer = fake_http(vec![json_reply(result), json_reply(result)]).await;
    let home = tempfile::tempdir().unwrap();
    let store = TokenStore::new(home.path(), "fixture");
    let target = TransportSpec::OAuth {
        url: peer.url.clone(),
        store: store.clone(),
    };
    let mut session = crate::transport::Transport::connect(&target).await.unwrap();
    let mut auth = StoredAuth {
        resource_url: Some(reqwest::Url::parse(&peer.url).unwrap().to_string()),
        ..Default::default()
    };
    for token in ["old", "rotated"] {
        auth.tokens = Some(StoredTokens {
            access_token: token.into(),
            refresh_token: None,
            expires_at: None,
        });
        store.save(&auth).unwrap();
        session.list_tools().await.unwrap();
    }
    auth.tokens.as_mut().unwrap().expires_at = Some(1);
    store.save(&auth).unwrap();
    assert!(session.list_tools().await.unwrap_err().contains("expired"));
    auth.resource_url = Some("https://unrelated.invalid/mcp".into());
    store.save(&auth).unwrap();
    assert!(session.list_tools().await.unwrap_err().contains("changed"));
    auth.tokens = None;
    auth.resource_url = None;
    store.save(&auth).unwrap();
    assert!(session.list_tools().await.unwrap_err().contains("required"));
    let seen = peer.seen.lock().await;
    assert_eq!(seen.len(), 2);
    assert!(seen[0].0.contains("Bearer old"));
    assert!(seen[1].0.contains("Bearer rotated"));
}

#[tokio::test]
async fn legacy_files_are_never_refreshed_in_the_background_and_login_owns_rotation() {
    use crate::oauth::{StoredTokens, TokenStore};
    let home = tempfile::tempdir().unwrap();
    let store = TokenStore::new(home.path(), "fixture");
    store
        .save_client_info(json!({"client_id":"fixture"}))
        .unwrap();
    store
        .save_tokens(StoredTokens {
            access_token: "old".into(),
            refresh_token: Some("mock".into()),
            expires_at: Some(1),
        })
        .unwrap();
    let lock = store.lock().unwrap();
    assert!(crate::renewal::refresh(&store, "http://127.0.0.1:1", true)
        .await
        .unwrap_err()
        .contains("concurrent"));
    drop(lock);
    assert!(crate::renewal::refresh(&store, "http://127.0.0.1:1", true)
        .await
        .unwrap_err()
        .contains("legacy"));
    assert_eq!(store.tokens().unwrap().access_token, "old");
}

#[tokio::test]
async fn a_refresh_reply_can_retain_the_existing_refresh_token() {
    use crate::oauth::{StoredAuth, StoredTokens, TokenStore};
    let peer = fake_http(vec![json_reply(
        r#"{"access_token":"new","expires_in":3600}"#,
    )])
    .await;
    let home = tempfile::tempdir().unwrap();
    let store = TokenStore::new(home.path(), "fixture");
    let url = format!("{}/mcp", peer.url);
    store
        .save(&StoredAuth {
            resource_url: Some(url.clone()),
            token_endpoint: Some(format!("{}/token", peer.url)),
            client_info: Some(json!({"client_id":"test"})),
            tokens: Some(StoredTokens {
                access_token: "old".into(),
                refresh_token: Some("keep".into()),
                expires_at: Some(1),
            }),
        })
        .unwrap();
    assert!(crate::renewal::refresh(&store, &url, true).await.unwrap());
    let rotated = store.tokens().unwrap();
    assert_eq!(rotated.access_token, "new");
    assert_eq!(rotated.refresh_token.as_deref(), Some("keep"));
}
