//! Native implementation of the Anthropic Messages API.
//!
//! Because loop's dialect is exactly Anthropic's shape, almost no conversion happens here:
//! the serde annotations on `Message` / `ContentBlock` (`tag = "type"` + snake_case)
//! make them serialize into exactly the wire form.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client as HttpClient;
use serde_json::{json, Value};

use crate::{
    sse, ContentBlock, CreateRequest, CreateResponse, Message, Provider, ProviderError, StopReason,
    TextSink, Usage,
};

const DEFAULT_BASE: &str = "https://api.anthropic.com";
/// Pinned version, not chasing new — an API version bump is a behavior change, hence explicit.
const API_VERSION: &str = "2023-06-01";

pub struct Client {
    http: HttpClient,
    base_url: String,
    api_key: String,
    /// Max retries on rate limits / transient faults (`JEEVES_LLM_RETRIES`).
    max_retries: u32,
    /// Main model and its requested manual thinking budget.
    thinking: Option<(String, u32)>,
}

impl Client {
    pub fn new(api_key: &str, base_url: Option<&str>, timeout: Duration, max_retries: u32) -> Self {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("build HTTP client");
        Self::with_http(api_key, base_url, http, max_retries)
    }
    pub(crate) fn with_http(
        api_key: &str,
        base_url: Option<&str>,
        http: reqwest::Client,
        max_retries: u32,
    ) -> Self {
        Self {
            http,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE)
                .trim_end_matches('/')
                .to_string(),
            api_key: api_key.to_string(),
            max_retries,
            thinking: None,
        }
    }

    /// Configure thinking for the main model. A budget of 0 uses its default.
    pub fn with_thinking(mut self, model: &str, budget_tokens: u32) -> Self {
        self.thinking = Some((model.to_string(), budget_tokens));
        self
    }

    #[cfg(test)]
    pub(crate) fn body_for_test(&self, request: &CreateRequest) -> Value {
        self.body(request, false)
    }

    fn body(&self, request: &CreateRequest, stream: bool) -> Value {
        let thinking = self
            .thinking
            .as_ref()
            .filter(|(model, _)| *model == request.model)
            .map(|(_, budget)| *budget);
        let mut body = anthropic_body(request, stream, thinking);
        if request.model == "claude-haiku-5-5"
            && self
                .thinking
                .as_ref()
                .is_some_and(|(main_model, _)| main_model != &request.model)
        {
            body["thinking"] = json!({"type": "disabled"});
            body["output_config"] = json!({"effort": "low"});
        }
        body
    }

    /// Send one request (retrying on rate limits / transient faults); returns the unread-body response.
    async fn send(&self, body: &Value) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}/v1/messages", self.base_url);
        crate::retry::with_retries(self.max_retries, || async {
            Ok(self
                .http
                .post(&url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", API_VERSION)
                .json(body)
                .send()
                .await?)
        })
        .await
    }

    /// Wrapper for one-shot calls.
    async fn post(&self, body: &Value) -> Result<String, ProviderError> {
        let response = self.send(body).await?;
        response.text().await.map_err(ProviderError::from)
    }
}

impl Provider for Client {
    fn create(
        &self,
        request: CreateRequest,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CreateResponse, ProviderError>> + Send + '_>,
    > {
        Box::pin(self.create_inner(request))
    }

    fn stream(
        &self,
        request: CreateRequest,
        on_text: TextSink,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CreateResponse, ProviderError>> + Send + '_>,
    > {
        Box::pin(self.stream_inner(request, on_text))
    }
}

/// Anthropic wants images wrapped in a `source` object:
/// `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}`. Every other block
/// is already wire-shaped (that is the serde-tag trick this wire leans on), so only Image
/// pays a conversion; the rest pass through `serde_json::to_value` byte-identically.
pub(crate) fn anthropic_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            json!({
                "role": m.role,
                "content": m
                    .content
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Image { media_type, data } => json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": media_type,
                                "data": data,
                            },
                        }),
                        other => serde_json::to_value(other)
                            .expect("ContentBlock is always serialisable"),
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// The system prompt as Anthropic wants it: a plain string, or two text blocks when there is
/// a stable prefix. The boundary between them is where `cache_control` goes — the soul and
/// the method paragraph are paid for once per session and read back at a fraction of the
/// price on every later turn. A tail that changes every turn stays outside the marked block.
fn anthropic_system(request: &CreateRequest) -> Option<Value> {
    let Some(prefix) = request.cache_prefix.as_ref().filter(|p| !p.is_empty()) else {
        return request.system.as_ref().map(|s| json!(s));
    };
    let mut blocks = vec![json!({
        "type": "text",
        "text": prefix,
        "cache_control": { "type": "ephemeral" },
    })];
    if let Some(tail) = request.system.as_ref().filter(|t| !t.is_empty()) {
        blocks.push(json!({ "type": "text", "text": tail }));
    }
    Some(Value::Array(blocks))
}

/// The second cache breakpoint: the last cacheable block of the last message. In a tool loop
/// every round re-sends the whole conversation; marking its tail lets the next round read the
/// history back from the cache instead of paying full input price again. Thinking blocks
/// cannot carry `cache_control`, so the mark goes on the last block that is not one.
/// Only applied when the request uses the system-prefix breakpoint too (the same opt-in).
fn mark_history_breakpoint(messages: &mut [Value]) {
    let Some(last) = messages.last_mut() else {
        return;
    };
    let Some(blocks) = last.get_mut("content").and_then(Value::as_array_mut) else {
        return;
    };
    if let Some(block) = blocks.iter_mut().rev().find(|b| {
        !matches!(
            b.get("type").and_then(Value::as_str),
            Some("thinking" | "redacted_thinking")
        ) && b.get("text").and_then(Value::as_str) != Some("")
    }) {
        block["cache_control"] = json!({ "type": "ephemeral" });
    }
}

/// The request body shared by `create` and `stream`.
fn anthropic_body(request: &CreateRequest, stream: bool, thinking: Option<u32>) -> Value {
    let mut messages = anthropic_messages(&request.messages);
    if request.cache_prefix.as_ref().is_some_and(|p| !p.is_empty()) {
        mark_history_breakpoint(&mut messages);
    }
    let mut body = json!({
        "model": request.model,
        "max_tokens": request.max_tokens,
        "messages": messages,
    });
    if stream {
        body["stream"] = json!(true);
    }
    if let Some(budget) = thinking
        .filter(|&budget| budget >= 1024 && i64::from(budget) < i64::from(request.max_tokens))
    {
        body["thinking"] = match request.model.as_str() {
            "claude-sonnet-5-5" | "claude-haiku-5-5" | "MiniMax-M3.1-Flash" | "MiniMax-M3" => {
                json!({"type": "adaptive"})
            }
            _ => json!({"type": "enabled", "budget_tokens": budget}),
        };
    }
    if let Some(system) = anthropic_system(request) {
        body["system"] = system;
    }
    if !request.tools.is_empty() {
        body["tools"] = json!(request.tools);
    }
    body
}

impl Client {
    async fn create_inner(&self, request: CreateRequest) -> Result<CreateResponse, ProviderError> {
        crate::reject_responses_continuation(&request)?;
        if request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|block| matches!(block, ContentBlock::OpenAiReasoning { .. }))
        {
            return Err(ProviderError::Api(
                "Chat Completions thinking continuation cannot be replayed through Anthropic"
                    .into(),
            ));
        }
        let body = self.body(&request, false);
        let text = self.post(&body).await?;

        let value: Value = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Parse(format!("the response is not valid JSON: {e}")))?;
        parse_message(&value)
    }

    /// SSE streaming. Text is called back as it arrives; tool-call arguments arrive in
    /// fragments (`partial_json` of `input_json_delta`), accumulated by index and parsed together.
    async fn stream_inner(
        &self,
        request: CreateRequest,
        on_text: TextSink,
    ) -> Result<CreateResponse, ProviderError> {
        crate::reject_responses_continuation(&request)?;
        if request
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|block| matches!(block, ContentBlock::OpenAiReasoning { .. }))
        {
            return Err(ProviderError::Api(
                "Chat Completions thinking continuation cannot be replayed through Anthropic"
                    .into(),
            ));
        }
        let body = self.body(&request, true);

        let response = self.send(&body).await?;

        let mut tools: BTreeMap<i64, (String, String, Value, String)> = BTreeMap::new();
        let mut thinking: BTreeMap<i64, ContentBlock> = BTreeMap::new();
        let mut text = String::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::EndTurn;
        let mut complete = false;

        let mut events = Box::pin(sse::data_lines(Box::pin(response.bytes_stream())));
        while let Some(item) = events.as_mut().next().await {
            let data = item?;
            let value: Value = serde_json::from_str(&data)
                .map_err(|e| ProviderError::Parse(format!("the SSE payload is not JSON: {e}")))?;

            match value.get("type").and_then(|t| t.as_str()) {
                Some("message_start") => {
                    usage.input_tokens = value
                        .pointer("/message/usage/input_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    usage.cache_read_input_tokens = value
                        .pointer("/message/usage/cache_read_input_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    usage.cache_creation_input_tokens = value
                        .pointer("/message/usage/cache_creation_input_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    usage.prompt_tokens = usage.input_tokens
                        + usage.cache_read_input_tokens
                        + usage.cache_creation_input_tokens;
                    usage.input_tokens = usage.prompt_tokens;
                }
                Some("content_block_start") => {
                    let index = value.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                    let block = value.get("content_block").cloned().unwrap_or(Value::Null);
                    let kind = block.get("type").and_then(|t| t.as_str());
                    if kind == Some("thinking") {
                        thinking.insert(
                            index,
                            ContentBlock::Thinking {
                                thinking: String::new(),
                                signature: String::new(),
                            },
                        );
                    } else if kind == Some("redacted_thinking") {
                        let data = block
                            .get("data")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        thinking.insert(index, ContentBlock::RedactedThinking { data });
                    } else if kind == Some("tool_use") {
                        let field = |key| {
                            block
                                .get(key)
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .ok_or_else(|| {
                                    ProviderError::Parse(format!("tool_use is missing {key}"))
                                })
                        };
                        let id = field("id")?.to_string();
                        let name = field("name")?.to_string();
                        tools.insert(index, (id, name, block["input"].clone(), String::new()));
                    }
                }
                Some("content_block_delta") => {
                    let index = value.get("index").and_then(|v| v.as_i64()).unwrap_or(0);
                    let delta = value.get("delta").cloned().unwrap_or(Value::Null);
                    match delta.get("type").and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            let piece = delta
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            if !piece.is_empty() {
                                text.push_str(piece);
                                on_text(piece);
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(entry) = tools.get_mut(&index) {
                                entry.3.push_str(
                                    delta
                                        .get("partial_json")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(ContentBlock::Thinking { thinking, .. }) =
                                thinking.get_mut(&index)
                            {
                                thinking.push_str(
                                    delta
                                        .get("thinking")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        Some("signature_delta") => {
                            if let Some(ContentBlock::Thinking { signature, .. }) =
                                thinking.get_mut(&index)
                            {
                                signature.push_str(
                                    delta
                                        .get("signature")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or_default(),
                                );
                            }
                        }
                        _ => {}
                    }
                }
                Some("message_delta") => {
                    if let Some(reason) =
                        value.pointer("/delta/stop_reason").and_then(|v| v.as_str())
                    {
                        stop_reason = super::anthropic::stop_reason(reason);
                    }
                    if let Some(out) = value
                        .pointer("/usage/output_tokens")
                        .and_then(|v| v.as_i64())
                    {
                        usage.output_tokens = out;
                    }
                }
                Some("error") => return Err(ProviderError::Api(value.to_string())),
                Some("message_stop") => {
                    complete = true;
                    break;
                }
                _ => {}
            }
        }

        if !complete {
            return Err(ProviderError::Parse(
                "Anthropic stream ended before message_stop".into(),
            ));
        }
        let mut content: Vec<ContentBlock> = thinking.into_values().collect();
        if !text.is_empty() {
            content.push(ContentBlock::Text { text });
        }
        for (_, (id, name, initial, arguments)) in tools {
            let input: Value = if arguments.is_empty() {
                initial
            } else {
                serde_json::from_str(&arguments)
                    .map_err(|e| ProviderError::Parse(format!("Invalid tool arguments: {e}")))?
            };
            if !input.is_object() {
                return Err(ProviderError::Parse("Tool input must be an object".into()));
            }
            content.push(ContentBlock::ToolUse {
                id,
                name,
                input,
                extra: None,
            });
        }

        Ok(CreateResponse {
            stop_reason,
            usage,
            content,
        })
    }
}

/// Extract the three things loop needs from an Anthropic response body: content blocks,
/// stop reason, usage. Split into its own function for unit testing — no real HTTP server.
pub fn parse_message(value: &Value) -> Result<CreateResponse, ProviderError> {
    let content: Vec<ContentBlock> =
        serde_json::from_value(value.get("content").cloned().unwrap_or(Value::Null))
            .map_err(|e| ProviderError::Parse(format!("bad content block: {e}")))?;

    let stop_reason = value
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .map(stop_reason)
        .unwrap_or(StopReason::EndTurn);

    let mut usage = value
        .get("usage")
        .cloned()
        .and_then(|u| serde_json::from_value::<Usage>(u).ok())
        .unwrap_or_default();
    usage.prompt_tokens =
        usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;
    usage.input_tokens = usage.prompt_tokens;

    Ok(CreateResponse {
        stop_reason,
        usage,
        content,
    })
}

pub fn stop_reason(wire: &str) -> StopReason {
    match wire {
        "end_turn" => StopReason::EndTurn,
        "tool_use" => StopReason::ToolUse,
        "max_tokens" => StopReason::MaxTokens,
        _ => StopReason::Other,
    }
}
