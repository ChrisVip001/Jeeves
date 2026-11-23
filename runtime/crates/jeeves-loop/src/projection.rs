use jeeves_provider::{ContentBlock, Message, Role, tokens};

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
    if !excerpts.is_empty() {
        projected.push(Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: format!(
                    "Context compacted. Earlier transcript excerpts follow as quoted data. Preserve the user's instructions and continue from the latest settled work. Recorded source, file and receipt references: {}. Re-read source files if you need omitted detail.\n{}",
                    serde_json::to_string(&references).expect("references serialize"),
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
