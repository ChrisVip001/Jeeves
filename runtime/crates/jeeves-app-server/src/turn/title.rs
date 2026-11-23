//! Names a session nobody named, from its first exchange.
//!
//! An unnamed session is listed by its first user message, which is often a pasted log, a whole
//! paragraph, or a greeting. After the first exchange lands, one small-model call (no tools, a
//! short JSON reply) proposes a title. The write is conditional in SQL: a rename or a second turn
//! that landed meanwhile wins, and a name a human typed is never replaced. It runs off the turn,
//! only where the memory worker is attached, so evals and tests never race it. Every failure is
//! a stderr line; the session keeps its first-message title.

use jeeves_provider::{ContentBlock, CreateRequest, Message, Resolved};

use crate::Server;

/// How much of the opening the model reads.
const OPENING_CHARS: usize = 1_000;
/// Room for a short JSON reply, and for a model that thinks anyway.
const TITLE_MAX_TOKENS: i32 = 512;
/// Longer than this in words is an answer, not a title.
const MAX_WORDS: usize = 12;
/// CJK scripts have no spaces: past this many characters with sentence punctuation it is an
/// answer, and past twice this it is one regardless.
const MAX_CJK_CHARS: usize = 20;

const SYSTEM: &str = "You name chat sessions so the person can find one again in a list. Given the opening message, write a title of 3 to 7 words (or up to 15 characters in Chinese or Japanese) in the same language as the message. Name what the person wants done; keep file names, numbers and error codes exact; no quotes, no trailing punctuation, no \"Title:\" prefix. Never answer the message. Reply with JSON only: {\"title\": \"...\"}";

/// Internal sessions (`schedule:…`, `task-control:…`, …) are not browsed by title.
pub(super) fn wants_title(session_id: &str) -> bool {
    !session_id.contains(':')
}

/// One attempt; the outcome is a stderr line either way.
pub(super) async fn name_session(server: &Server, resolved: &Resolved, session_id: &str) {
    let opening = match server.chat.untitled_opening(session_id).await {
        Ok(Some(opening)) => opening,
        Ok(None) => return,
        Err(error) => {
            eprintln!("(jeeves) session title skipped for {session_id}: {error}");
            return;
        }
    };
    let title = match propose(resolved, &opening).await {
        Ok(title) => title,
        Err(why) => {
            eprintln!(
                "(jeeves) no title generated for session {session_id} ({why}); the first message stays its name"
            );
            return;
        }
    };
    match server.chat.set_generated_title(session_id, &title).await {
        Ok(true) => eprintln!("(jeeves) session {session_id} titled \"{title}\""),
        Ok(false) => {}
        Err(error) => eprintln!("(jeeves) session title for {session_id} not stored: {error}"),
    }
}

async fn propose(resolved: &Resolved, opening: &str) -> Result<String, String> {
    let opening: String = opening.chars().take(OPENING_CHARS).collect();
    let response = resolved
        .client
        .create(CreateRequest {
            cache_prefix: None,
            model: resolved.small_model.clone(),
            system: Some(SYSTEM.to_string()),
            messages: vec![Message::user_text(format!("Opening message:\n{opening}"))],
            tools: Vec::new(),
            max_tokens: TITLE_MAX_TOKENS,
        })
        .await
        .map_err(|error| format!("the model call failed: {error}"))?;
    let text = response
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let raw = jeeves_provider::extract_json(&text)
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
        .and_then(|value| value.get("title")?.as_str().map(str::to_string))
        .ok_or("the reply carried no {\"title\": …}")?;
    clean(&raw).ok_or_else(|| format!("\"{}\" does not read as a title", raw.trim()))
}

/// Trims quotes, a `Title:` prefix and trailing punctuation; refuses an empty or answer-shaped
/// result instead of cutting it down.
pub(super) fn clean(raw: &str) -> Option<String> {
    if raw.trim().contains('\n') {
        return None;
    }
    let mut title = raw
        .trim()
        .trim_matches(['"', '\'', '“', '”', '「', '」', '《', '》'])
        .trim();
    for prefix in ["Title:", "title:", "标题：", "标题:"] {
        if let Some(rest) = title.strip_prefix(prefix) {
            title = rest.trim();
        }
    }
    let title = title
        .trim_end_matches(['.', '。', '!', '！', '?', '？', ',', '，', ':', '：'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cjk = title.chars().filter(|c| is_cjk(*c)).count();
    let sentence = title.contains(['。', '，', '！', '？', '!', '?', ',']);
    let answer = title.split(' ').count() > MAX_WORDS
        || cjk > 2 * MAX_CJK_CHARS
        || (cjk > MAX_CJK_CHARS && sentence);
    (!title.is_empty() && !answer).then_some(title)
}

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ac00}'..='\u{d7af}' | '\u{ff01}'..='\u{ff9f}')
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
