//! Records turn events for the ledger and memory center.
//!
//! Conversation history comes from `chat_log`. The ledger retains details such as tool calls
//! and results for review.

//! **Big payloads do not go into the row.** A 200 KB tool result would bloat the
//! database and every backup; instead it lands in `<home>/ledger/<session>/…` and the
//! row keeps a pointer plus the content digest, so verification still works and the
//! text is one file read away.
//!
//! Failures here never fail the turn: the reply is already persisted by the time this
//! runs, and losing the ledger row is a reporting loss, not a conversational one —
//! exactly the discipline tool errors follow.

use std::path::{Path, PathBuf};

use jeeves_provider::{ContentBlock, Message, Role};
use jeeves_state::NewEntry;
use serde_json::json;

/// Where spilled payloads live and how large a payload may be before it is spilled.
#[derive(Debug, Clone, Copy)]
pub struct LedgerSink<'a> {
    pub home: &'a Path,
    pub inline_max: usize,
}

impl<'a> LedgerSink<'a> {
    pub fn new(home: &'a Path, inline_max: usize) -> Self {
        Self { home, inline_max }
    }

    fn dir(&self) -> PathBuf {
        self.home.join("ledger")
    }
}

/// One content block's payload: either the text itself or a pointer to the spilled file.
fn payload_of(
    sink: &LedgerSink<'_>,
    session_id: &str,
    turn_id: &str,
    ordinal: usize,
    kind: &str,
    body: &str,
    always_spill: bool,
) -> String {
    if !always_spill && body.len() <= sink.inline_max {
        return body.to_string();
    }
    let ext = if kind == "tool_use" { "json" } else { "txt" };
    let name = format!("{turn_id}-{ordinal:02}-{kind}.{ext}");
    let dir = sink.dir().join(safe_component(session_id));
    let path = dir.join(&name);
    let digest = jeeves_state::ledger::digest_of(body.as_bytes());
    let bytes = body.len();
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, body.as_bytes())) {
        Ok(()) => json!({
            "ptr": format!("ledger/{}/{}", safe_component(session_id), name),
            "digest": digest,
            "bytes": bytes,
            "preview": body.chars().take(200).collect::<String>(),
        })
        .to_string(),
        Err(error) => {
            eprintln!(
                "(jeeves) ledger spill failed; storing whole payloads instead ({}): {error}",
                path.display()
            );
            body.to_string()
        }
    }
}

/// Session ids are labels; keep them usable as directory names without letting `..` out.
fn safe_component(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

/// Build the ledger rows for one finished turn.
///
/// `user_message` is recorded once from the caller (it is also inside `messages`, so the
/// matching copy is skipped so the ledger shows the request once).
pub fn entries_for_turn(
    sink: &LedgerSink<'_>,
    session_id: &str,
    turn_id: &str,
    user_message: &str,
    messages: &[Message],
    reply: &str,
) -> Vec<NewEntry> {
    let mut entries: Vec<NewEntry> = Vec::new();
    let mut ordinal = 0usize;

    entries.push(NewEntry::exact(
        "user",
        payload_of(
            sink,
            session_id,
            turn_id,
            0,
            "user",
            &json!({ "text": user_message }).to_string(),
            false,
        ),
    ));

    let mut saw_assistant = false;
    let mut user_recorded = false;
    for message in messages {
        match message.role {
            Role::Assistant => {
                saw_assistant = true;
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } if !text.trim().is_empty() => {
                            let body = json!({ "text": text }).to_string();
                            entries.push(NewEntry::exact(
                                "assistant",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "assistant",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            let body =
                                json!({ "id": id, "name": name, "input": input }).to_string();
                            entries.push(NewEntry::exact(
                                "tool_use",
                                payload_of(
                                    sink, session_id, turn_id, ordinal, "tool_use", &body, false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::Image { media_type, data } => {
                            let body = json!({ "mediaType": media_type, "data": data }).to_string();
                            entries.push(NewEntry::exact(
                                "image",
                                payload_of(
                                    sink, session_id, turn_id, ordinal, "image", &body, true,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::OpenAiReasoning { text } => {
                            let body =
                                json!({ "wire": "openai", "reasoningContent": text }).to_string();
                            entries.push(NewEntry::exact(
                                "provider_item",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "provider_item",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::ResponseItem { item } => {
                            let body = json!({ "wire": "responses", "item": item }).to_string();
                            entries.push(NewEntry::exact(
                                "provider_item",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "provider_item",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        _ => {}
                    }
                }
            }
            Role::User => {
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !user_recorded && text == user_message {
                                user_recorded = true;
                                continue;
                            }
                            if text.trim().is_empty() {
                                continue;
                            }
                            let body = json!({ "text": text }).to_string();
                            entries.push(NewEntry::exact(
                                "user",
                                payload_of(
                                    sink, session_id, turn_id, ordinal, "user", &body, false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                        } => {
                            let body =
                                json!({ "toolUseId": tool_use_id, "content": content }).to_string();
                            entries.push(NewEntry::exact(
                                "tool_result",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "tool_result",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::Image { media_type, data } => {
                            let body = json!({ "mediaType": media_type, "data": data }).to_string();
                            entries.push(NewEntry::exact(
                                "image",
                                payload_of(
                                    sink, session_id, turn_id, ordinal, "image", &body, true,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::OpenAiReasoning { text } => {
                            let body =
                                json!({ "wire": "openai", "reasoningContent": text }).to_string();
                            entries.push(NewEntry::exact(
                                "provider_item",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "provider_item",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::ResponseItem { item } => {
                            let body = json!({ "wire": "responses", "item": item }).to_string();
                            entries.push(NewEntry::exact(
                                "provider_item",
                                payload_of(
                                    sink,
                                    session_id,
                                    turn_id,
                                    ordinal,
                                    "provider_item",
                                    &body,
                                    false,
                                ),
                            ));
                            ordinal += 1;
                        }
                        ContentBlock::ToolUse { .. }
                        | ContentBlock::Thinking { .. }
                        | ContentBlock::RedactedThinking { .. } => {}
                    }
                }
            }
        }
    }

    if !saw_assistant && !reply.trim().is_empty() {
        let body = json!({ "text": reply }).to_string();
        entries.push(NewEntry::exact(
            "assistant",
            payload_of(
                sink,
                session_id,
                turn_id,
                ordinal,
                "assistant",
                &body,
                false,
            ),
        ));
    }
    entries
}

#[cfg(test)]
#[path = "ledger_writer_tests.rs"]
mod ledger_writer_tests;
