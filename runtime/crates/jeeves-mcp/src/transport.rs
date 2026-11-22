//! Two transports, one request interface.
//!
//! stdio: the client spawns the server as a local subprocess, one JSON-RPC message per line.
//! Streamable HTTP: the server is already running elsewhere; POST to it.
//!
//! Both paths do one thing: turn `request(method, params)` into `result`.
//! The rest (who connects, what tools are called) lives above.
//!
//! A session is **exclusive**: a request goes out and we read until the matching answer.
//! So every connection is wrapped in a Mutex (see `lib.rs`) — concurrent calls queue
//! instead of eating each other's answers.

use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// MCP protocol version. The server may answer with its own supported version; if not it
/// still answers — we only use the `tools` capability, which version differences don't touch.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const CLIENT_NAME: &str = "jeeves";

/// Per-request cap. Tool calls included — a stuck server must not stall a whole turn.
pub const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REPLY_BYTES: usize = 2 * 1024 * 1024;

/// One tool reported by the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ToolMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// The wire field is `inputSchema`; the SDK 2.x Python port calls it `input_schema`.
    /// Accept both spellings, so a single underscore cannot break the connection.
    #[serde(default, rename = "inputSchema", alias = "input_schema")]
    pub input_schema: Option<Value>,
}

impl ToolMeta {
    /// Schema for the model: if the server gave none, degrade to "an object with no parameters".
    pub fn schema(&self) -> Value {
        match &self.input_schema {
            Some(Value::Object(_)) => self.input_schema.clone().unwrap(),
            _ => json!({"type": "object", "properties": {}}),
        }
    }
}

/// A session speaking one JSON-RPC message per line.
///
/// The generics exist so tests can use `tokio::io::duplex` as the pipe — "is the protocol
/// right" can then be tested without spawning a real subprocess.
pub struct StdioSession<R, W> {
    reader: BufReader<R>,
    writer: W,
    next_id: u64,
}

impl<R, W> StdioSession<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub fn new(reader: R, writer: W) -> Self {
        StdioSession {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
        }
    }

    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(message).map_err(|e| e.to_string())?;
        line.push('\n');
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("could not write: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| format!("could not write: {e}"))
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.send(&message).await
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send(&message).await?;

        loop {
            let mut line = String::new();
            let read = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(|e| format!("could not read: {e}"))?;
            if read == 0 {
                return Err("the server closed the connection".to_string());
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                if let (Some(method), Some(their_id)) = (
                    message.get("method").and_then(Value::as_str),
                    message.get("id").cloned(),
                ) {
                    eprintln!("(jeeves) the MCP server sent a reverse request '{method}'; refused explicitly (not supported)");
                    let reply = json!({
                        "jsonrpc": "2.0",
                        "id": their_id,
                        "error": {
                            "code": -32601,
                            "message": format!("Jeeves does not support the reverse request '{method}'"),
                        },
                    });
                    self.send(&reply).await?;
                }
                continue;
            }
            return unwrap_message(&message);
        }
    }
}

/// Handshake, list tools and call tools are identical across both transports, so they live
/// on the trait; each framing only handles "how to get one message out and back".
///
/// Method names carry the `rpc_` prefix to separate them from the two structs' own
/// `request` / `notify` — those target concrete types; no same-named parsing rules.
///
/// The `async_fn_in_trait` lint targets "handing a trait to others to implement"; here
/// only the two implementations in this file exist and it is never used as `dyn`, so the
/// Future needs no `Send` bound.
#[allow(async_fn_in_trait)]
pub trait Rpc {
    async fn rpc_request(&mut self, method: &str, params: Value) -> Result<Value, String>;
    async fn rpc_notify(&mut self, method: &str, params: Value) -> Result<(), String>;
}

impl<R, W> Rpc for StdioSession<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    async fn rpc_request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        StdioSession::request(self, method, params).await
    }

    async fn rpc_notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        StdioSession::notify(self, method, params).await
    }
}

impl Rpc for HttpSession {
    async fn rpc_request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        HttpSession::request(self, method, params).await
    }

    async fn rpc_notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        HttpSession::notify(self, method, params).await
    }
}

fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
    })
}

/// Handshake: announce who we are, get the server's capabilities, then say "we're ready".
pub async fn initialize(session: &mut impl Rpc) -> Result<Value, String> {
    let result = session
        .rpc_request("initialize", initialize_params())
        .await?;
    session
        .rpc_notify("notifications/initialized", json!({}))
        .await?;
    Ok(result)
}

pub async fn list_tools(session: &mut impl Rpc) -> Result<Vec<ToolMeta>, String> {
    let result = session.rpc_request("tools/list", json!({})).await?;
    #[derive(serde::Deserialize)]
    struct List {
        tools: Vec<ToolMeta>,
    }
    serde_json::from_value::<List>(result)
        .map(|list| list.tools)
        .map_err(|e| format!("the tool list could not be read: {e}"))
}

/// Call a tool once, returning the text for the model to read.
pub async fn call_tool(session: &mut impl Rpc, tool: &str, args: Value) -> Result<String, String> {
    let result = session
        .rpc_request("tools/call", json!({"name": tool, "arguments": args}))
        .await?;
    let mut parts: Vec<String> = Vec::new();
    let blocks = match result.get("content") {
        Some(Value::Array(blocks)) => blocks.as_slice(),
        None if result.get("structuredContent").is_some() => &[],
        _ => return Err("MCP tool result has no valid content".into()),
    };
    for block in blocks {
        let text = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .unwrap_or("[non-text content]");
        parts.push(text.to_string());
    }
    let joined = parts.join("\n");
    let joined = if joined.is_empty() {
        result
            .get("structuredContent")
            .map(Value::to_string)
            .unwrap_or_else(|| "(no output)".to_string())
    } else {
        joined
    };
    if result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(format!("Error: MCP tool {tool} failed: {joined}"));
    }
    Ok(joined)
}

impl<R, W> StdioSession<R, W>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    pub async fn initialize(&mut self) -> Result<Value, String> {
        initialize(self).await
    }

    pub async fn list_tools(&mut self) -> Result<Vec<ToolMeta>, String> {
        list_tools(self).await
    }

    pub async fn call_tool(&mut self, tool: &str, args: Value) -> Result<String, String> {
        call_tool(self, tool, args).await
    }
}

/// Take `result` out of a JSON-RPC message, or turn `error` into an error.
fn unwrap_message(message: &Value) -> Result<Value, String> {
    if let Some(error) = message.get("error") {
        if !error.is_null() {
            let text = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("(no description)");
            let code = error.get("code").map(Value::to_string).unwrap_or_default();
            return Err(format!("{text} ({code})"));
        }
    }
    message
        .get("result")
        .cloned()
        .ok_or_else(|| "JSON-RPC reply is missing result or error".into())
}

/// Streamable HTTP session. The old HTTP+SSE transport is deprecated; unsupported on purpose.
pub struct HttpSession {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
    oauth: Option<crate::oauth::TokenStore>,
    /// Given by the server in the initialize answer; every later request must carry it back.
    session_id: Option<String>,
    /// Set when a request that carried a session id was answered 404: the server forgot it.
    session_expired: bool,
    next_id: u64,
}

impl HttpSession {
    pub fn new(url: &str, token: Option<String>) -> Self {
        HttpSession {
            client: reqwest::Client::new(),
            url: url.to_string(),
            token,
            oauth: None,
            session_id: None,
            session_expired: false,
            next_id: 1,
        }
    }

    fn authorize(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, String> {
        if let Some(store) = &self.oauth {
            Ok(request.bearer_auth(store.access(&self.url)?))
        } else if let Some(token) = &self.token {
            Ok(request.bearer_auth(token))
        } else {
            Ok(request)
        }
    }

    async fn post(&mut self, body: Value) -> Result<(u16, String, String), String> {
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .json(&body);
        request = self.authorize(request)?;
        if let Some(session) = &self.session_id {
            request = request.header("mcp-session-id", session);
            request = request.header("mcp-protocol-version", PROTOCOL_VERSION);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("could not send the request: {e}"))?;
        let status = response.status().as_u16();
        if status == 403
            && response
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.split(',').any(|part| {
                        let part = part.trim();
                        let part = part.strip_prefix("Bearer ").unwrap_or(part);
                        matches!(
                            part,
                            "error=\"insufficient_scope\"" | "error=insufficient_scope"
                        )
                    })
                })
        {
            return Err("HTTP 403: OAuth insufficient_scope".into());
        }
        if let Some(session) = response.headers().get("mcp-session-id") {
            if let Ok(session) = session.to_str() {
                self.session_id = Some(session.to_string());
            }
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut response = response;
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("could not read the reply: {e}"))?
        {
            if chunk.len() > MAX_REPLY_BYTES - body.len() {
                return Err("MCP HTTP reply exceeds the 2 MiB limit".into());
            }
            body.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&body).into_owned();
        Ok((status, content_type, text))
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let (status, _, text) = self
            .post(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await?;
        if status == 202 || text.trim().is_empty() {
            return Ok(());
        }
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}: {}", first_line(&text)));
        }
        Ok(())
    }

    /// One request. When the server has dropped our session (HTTP 404 on a request that carried
    /// a session id), the spec says to start a new one: forget the id, handshake again, and send
    /// the request once more. A 404 means the server rejected the request before running it, so
    /// the retry cannot repeat a side effect.
    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let first = self.request_once(method, params.clone()).await;
        if first.is_err() && self.session_expired && method != "initialize" {
            self.session_expired = false;
            self.session_id = None;
            self.request_once("initialize", initialize_params()).await?;
            self.notify("notifications/initialized", json!({})).await?;
            return self.request_once(method, params).await;
        }
        first
    }

    /// End the session politely (spec: `DELETE` with the session id). Failures are ignored on
    /// purpose: servers may answer 405, and the caller is going away either way.
    pub async fn close(&mut self) {
        let Some(session) = self.session_id.take() else {
            return;
        };
        let request = self
            .client
            .delete(&self.url)
            .header("mcp-session-id", session)
            .header("mcp-protocol-version", PROTOCOL_VERSION);
        let Ok(request) = self.authorize(request) else {
            return;
        };
        let _ = tokio::time::timeout(std::time::Duration::from_secs(3), request.send()).await;
    }

    async fn request_once(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let carried_session = self.session_id.is_some();
        let (status, content_type, text) = self
            .post(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        if status == 404 && carried_session {
            self.session_expired = true;
        }
        if !(200..300).contains(&status) {
            return Err(format!("HTTP {status}: {}", first_line(&text)));
        }
        if content_type.contains("text/event-stream") {
            let message = find_in_stream(&text, id).ok_or_else(|| {
                "the SSE stream does not contain the reply we asked for".to_string()
            })?;
            return unwrap_message(&message);
        }
        if text.trim().is_empty() {
            return Err("the server returned an empty body".to_string());
        }
        let message: Value =
            serde_json::from_str(&text).map_err(|e| format!("the reply is not JSON: {e}"))?;
        match message {
            Value::Array(messages) => messages
                .iter()
                .find(|m| m.get("id").and_then(Value::as_u64) == Some(id))
                .map(unwrap_message)
                .unwrap_or_else(|| {
                    Err("the reply does not contain the answer we asked for".to_string())
                }),
            other => unwrap_message(&other),
        }
    }

    pub async fn initialize(&mut self) -> Result<Value, String> {
        initialize(self).await
    }

    pub async fn list_tools(&mut self) -> Result<Vec<ToolMeta>, String> {
        list_tools(self).await
    }

    pub async fn call_tool(&mut self, tool: &str, args: Value) -> Result<String, String> {
        call_tool(self, tool, args).await
    }
}

/// Find the message matching `id` inside an SSE stream.
fn find_in_stream(text: &str, id: u64) -> Option<Value> {
    for line in text.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(message) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) {
            return Some(message);
        }
    }
    None
}

/// On server errors the body can be a whole page of HTML. Only the first line fits in a log.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    line.chars().take(200).collect()
}

/// A transport that is already connected.
pub enum Transport {
    Stdio(Box<StdioTransport>),
    Http(HttpSession),
}

/// The two handles of the stdio path: the pipe session + the subprocess itself.
///
/// Boxed — it is an order of magnitude larger than `HttpSession`; left in the enum variant
/// every `Transport` would occupy the largest variant's size.
pub struct StdioTransport {
    session: StdioSession<ChildStdout, ChildStdin>,
    /// The subprocess handle, only to keep it alive (kill_on_drop takes it away on drop).
    _child: Child,
}

impl Transport {
    /// Start a transport from config. **Not yet** handshaken — that is [`Transport::initialize`].
    pub async fn connect(spec: &crate::config::Transport) -> Result<Transport, String> {
        match spec {
            crate::config::Transport::Stdio { command, args, env } => {
                let mut child = Command::new(command);
                child
                    .args(args)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true);
                if let Some(env) = env {
                    child.env_clear();
                    child.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                }
                let mut child = child
                    .spawn()
                    .map_err(|e| format!("could not start '{command}': {e}"))?;
                let stdin = child
                    .stdin
                    .take()
                    .ok_or("could not get the child's stdin")?;
                let stdout = child
                    .stdout
                    .take()
                    .ok_or("could not get the child's stdout")?;
                Ok(Transport::Stdio(Box::new(StdioTransport {
                    session: StdioSession::new(stdout, stdin),
                    _child: child,
                })))
            }
            crate::config::Transport::OAuth { url, store } => {
                let mut session = HttpSession::new(url, None);
                session.oauth = Some(store.clone());
                Ok(Transport::Http(session))
            }
            crate::config::Transport::Http { url, token } => {
                Ok(Transport::Http(HttpSession::new(url, token.clone())))
            }
        }
    }

    /// Tell the server we are done. Only HTTP has a session to end; a stdio child dies with us.
    pub async fn close(&mut self) {
        if let Transport::Http(session) = self {
            session.close().await;
        }
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Transport::Stdio(stdio) => stdio.session.request(method, params).await,
            Transport::Http(session) => session.request(method, params).await,
        }
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Transport::Stdio(stdio) => stdio.session.notify(method, params).await,
            Transport::Http(session) => session.notify(method, params).await,
        }
    }

    pub async fn initialize(&mut self) -> Result<Value, String> {
        match self {
            Transport::Stdio(stdio) => stdio.session.initialize().await,
            Transport::Http(session) => session.initialize().await,
        }
    }

    pub async fn list_tools(&mut self) -> Result<Vec<ToolMeta>, String> {
        match self {
            Transport::Stdio(stdio) => stdio.session.list_tools().await,
            Transport::Http(session) => session.list_tools().await,
        }
    }

    /// Call a tool once, returning the text for the model to read.
    pub async fn call_tool(&mut self, tool: &str, args: Value) -> Result<String, String> {
        match self {
            Transport::Stdio(stdio) => stdio.session.call_tool(tool, args).await,
            Transport::Http(session) => session.call_tool(tool, args).await,
        }
    }
}
