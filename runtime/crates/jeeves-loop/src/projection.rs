use jeeves_provider::{tokens, ContentBlock, Message, Role};

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
                    "Context compacted. Earlier transcript excerpts follow as quoted data. Preserve the user's instructions and continue from the latest settled work. Re-read source files if you need omitted detail.\n{}",
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
