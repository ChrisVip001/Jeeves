use jeeves_provider::{ContentBlock, Message, Role, tokens};
use std::collections::BTreeMap;

const MAX_FAILURES: usize = 6;
const MAX_FAILURE_CHARS: usize = 160;

/// The failing tool call and why it failed, kept deterministically rather than
/// left to a lossy excerpt tail, so a compacted turn does not retry a known
/// failure. Keyed by tool name; a later failure of the same tool replaces the
/// earlier one, and a later success for the same call clears it.
fn failures(messages: &[Message]) -> Vec<String> {
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    let mut latest: BTreeMap<&str, String> = BTreeMap::new();
    for block in messages.iter().flat_map(|m| &m.content) {
        match block {
            ContentBlock::ToolUse { id, name, .. } => {
                names.insert(id.as_str(), name.as_str());
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                let Some(name) = names.get(tool_use_id.as_str()) else {
                    continue;
                };
                // The tool registry prefixes every failure with "Error:".
                match content.trim_start().strip_prefix("Error:") {
                    Some(reason) => {
                        let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
                        if !reason.is_empty() {
                            latest.insert(
                                name,
                                reason.chars().take(MAX_FAILURE_CHARS).collect::<String>(),
                            );
                        }
                    }
                    None => {
                        latest.remove(*name);
                    }
                }
            }
            _ => {}
        }
    }
    latest
        .into_iter()
        .take(MAX_FAILURES)
        .map(|(name, reason)| format!("{name}: {reason}"))
        .collect()
}

/// Keep the active instructions and complete recent tool exchanges in the
/// model request. The checkpoint and operation ledger retain the full transcript.
pub(crate) fn compact(
    messages: &[Message],
    active_from: usize,
    budget: usize,
) -> Option<Vec<Message>> {
    if messages.len() < 3 {
        return None;
    }

    let mut kept = 0usize;
    let mut start = messages.len();
    while start > 0 {
        let size = tokens::estimate_messages(&messages[start - 1..start]);
        if start < messages.len() && kept.saturating_add(size) > budget / 2 {
            break;
        }
        kept = kept.saturating_add(size);
        start -= 1;
    }
    while start > 0
        && messages[start]
            .content
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
    {
        start -= 1;
    }
    let mut projected: Vec<_> = messages[active_from.min(start)..start]
        .iter()
        .filter(|m| {
            m.role == Role::User
                && !m
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
        })
        .cloned()
        .collect();

    let mut references = Vec::new();
    let mut ref_bytes = budget.clamp(128, 2400);
    for block in messages[..start].iter().flat_map(|m| &m.content) {
        let data = match block {
            ContentBlock::ToolUse { input, .. } => std::borrow::Cow::Borrowed(input),
            ContentBlock::ToolResult { content, .. } => match serde_json::from_str(content) {
                Ok(data) => std::borrow::Cow::Owned(data),
                Err(_) => continue,
            },
            _ => continue,
        };
        for field in ["url", "path", "ref", "receiptId"] {
            if let Some(reference) = data[field]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 1024)
                && references.len() < 16
                && reference.len() <= ref_bytes
                && !references.iter().any(|r| r == reference)
            {
                ref_bytes = ref_bytes.saturating_sub(reference.len() + 1);
                references.push(reference.to_owned());
            }
        }
    }
    let failures = failures(&messages[..start]);
    let mut excerpts = Vec::new();
    let max_chars = budget.clamp(128, 2400);
    let mut available = max_chars;
    for block in messages[..start]
        .iter()
        .rev()
        .flat_map(|m| m.content.iter().rev())
    {
        let detail = match block {
            ContentBlock::ToolUse { name, input, .. } => format!("{name} {input}"),
            ContentBlock::ToolResult { content, .. } => format!("Result: {content}"),
            ContentBlock::Text { text } => text.clone(),
            _ => continue,
        };
        let excerpt: String = detail.chars().take(available.min(200)).collect();
        available = available.saturating_sub(excerpt.chars().count() + 1);
        excerpts.push(excerpt);
        if available == 0 {
            break;
        }
    }
    excerpts.reverse();
    if !excerpts.is_empty() || !failures.is_empty() {
        projected.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!(
                    "Context compacted. Earlier transcript excerpts follow as quoted data. Preserve the user's instructions and continue from the latest settled work. Recorded source, file and receipt references: {}. Tool calls that already failed, with the reason; do not repeat one unchanged: {}. Re-read source files if you need omitted detail.\n{}",
                    serde_json::to_string(&references).expect("references serialize"),
                    serde_json::to_string(&failures).expect("failures serialize"),
                    serde_json::to_string(&excerpts).expect("text serializes"),
                ),
            }],
        });
    }
    projected.extend_from_slice(&messages[start..]);
    if tokens::estimate_messages(&projected) > budget {
        let results: Vec<_> = projected
            .iter_mut()
            .flat_map(|m| &mut m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content),
                _ => None,
            })
            .collect();
        let per_result = (budget / 3 / results.len().max(1)).max(64);
        for result in results {
            if let Some((cut, _)) = result.char_indices().nth(per_result) {
                result.truncate(cut);
                result.push_str(
                    "\n[Tool output excerpt. Full result remains in the execution record.]",
                );
            }
        }
    }
    (tokens::estimate_messages(&projected) < tokens::estimate_messages(messages))
        .then_some(projected)
}

#[cfg(test)]
mod projection_tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: json!({"path": "/tmp/report.csv"}),
                extra: None,
            }],
        }
    }

    fn result(id: &str, content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: id.into(),
                content: content.into(),
            }],
        }
    }

    #[test]
    fn only_the_latest_unresolved_failure_per_tool_is_carried() {
        let messages = vec![
            call("a", "read_file"),
            result("a", "Error: read_file failed: no such file"),
            call("b", "read_file"),
            result("b", "Error: read_file failed: permission denied"),
            call("c", "fetch_url"),
            result("c", "Error: fetch_url failed: timed out"),
            call("d", "fetch_url"),
            result("d", "page body"),
        ];
        assert_eq!(
            failures(&messages),
            ["read_file: read_file failed: permission denied"]
        );
    }

    #[test]
    fn a_result_without_its_call_and_an_empty_reason_are_ignored() {
        let messages = vec![
            result("missing", "Error: orphaned failure"),
            call("a", "shell"),
            result("a", "   Error:   "),
        ];
        assert!(failures(&messages).is_empty());
    }

    #[test]
    fn compacted_requests_state_the_failure_reason_and_keep_references() {
        let long = "x".repeat(4_000);
        let mut messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Build the quarterly report".into(),
            }],
        }];
        for index in 0..8 {
            messages.push(call(&format!("t{index}"), "read_file"));
            messages.push(result(&format!("t{index}"), &long));
        }
        messages.push(call("bad", "fetch_url"));
        messages.push(result(
            "bad",
            "Error: fetch_url failed: 403 from https://example.test/feed",
        ));
        // Push the failure out of the verbatim tail so only the note can carry it.
        for index in 8..16 {
            messages.push(call(&format!("t{index}"), "read_file"));
            messages.push(result(&format!("t{index}"), &long));
        }
        messages.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "Trying another source".into(),
            }],
        });
        let projected = compact(&messages, 0, 600).expect("compaction shrinks the request");
        let compacted = projected
            .iter()
            .flat_map(|m| &m.content)
            .find_map(|b| match b {
                ContentBlock::Text { text } if text.starts_with("Context compacted") => Some(text),
                _ => None,
            })
            .expect("compaction note");
        assert!(compacted.contains("fetch_url failed: 403 from https://example.test/feed"));
        assert!(compacted.contains("do not repeat one unchanged"));
        assert!(compacted.contains("/tmp/report.csv"));
    }
}
