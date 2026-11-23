//! Context compaction: conversation beyond the sliding window, folded back into the prompt.
//!
//! The sliding window (`JEEVES_HISTORY_TURNS`) is a hard boundary: older turns no longer
//! enter the prompt. Fine for short sessions; amnesia for long ones — "what did we
//! discuss last week" vanishes from context. Compaction **folds the evicted part
//! into one summary**, stored per session in state.db and spliced into the system
//! prompt every turn. The summary rolls forward: only newly evicted turns, no full recompute.
//!
//! Three rules, consistent with the rest of this crate:
//!
//! * **Fail-open**: a failed summary call falls back to the **deterministic fallback**
//!   (truncated concatenation of evicted turns); content is never lost to a stubborn
//!   model — a rough summary beats a conversation vanishing into thin air.
//! * **Roll forward only**: covered turns aren't recomputed. The summary saves tokens;
//!   it isn't a second memory source; the truth stays chat_log in state.db.
//! * **Observable**: the caller gets `(covered turns, summary)` — where it ends is visible.
//!
//! Tool output needs no trimming here: writes already pass through `fold_tool_activity`
//! into one truncated line (see app-server's turn.rs) — history has no huge blocks.

use anyhow::Result;
use jeeves_provider::{CreateRequest, Message, Provider};

/// Char cap for the fallback summary. Enough for a few turns' skeleton, without becoming a bomb.
const FALLBACK_CHARS: usize = 600;

/// The summarizer prompt. The goal is **keeping facts**, not literary polish.
///
/// Fixed sections, because a free paragraph loses the two things a resumed conversation needs
/// most: which requests are still open, and which actions already happened (a model that only
/// reads "we discussed the email" sends it again). Each section may be left out when empty, so
/// a short chat stays a few lines.
pub(crate) const SUMMARIZER_PROMPT: &str = "\
You are compressing the older part of a conversation between a user and their \
assistant, so it can stay in the context window. Write in the language of the \
conversation, using these sections in this order and leaving out any that would \
be empty:

Goal: what the user is trying to achieve, in one or two lines.
Done: actions already completed, as past-tense facts with their outcome \
(\"Sent the draft to Lin; she approved it\"), so they are not repeated.
Decisions and preferences: what was decided or what the user asked for, with \
the reason when one was given.
Open: requests not yet answered or work not yet finished, quoting the user's \
words where they matter.
Facts: names, numbers, dates, paths, links and identifiers, exactly as written.

Use short single-line bullets. When an earlier summary is given, keep everything \
in it that is still true, move finished items from Open to Done, and let the \
newer turns win where they disagree. Drop pleasantries and repetition. No \
commentary about the task of summarizing.";

/// Folds newly evicted turns into the existing summary. **Never returns Err** — the
/// deterministic fallback covers model failures.
///
/// Returns `(new covered count, new summary)`. `previous` is the last summary
/// (covered turns + body); `newly_evicted` is the old turns evicted this time.
pub async fn roll_forward(
    client: &dyn Provider,
    model: &str,
    previous: Option<(i32, String)>,
    newly_evicted: &[(String, String)],
) -> (i32, String) {
    roll_forward_with(client, model, previous, newly_evicted, None).await
}

/// Same, with a human's emphasis (`/compact keep the migration plan in view`).
///
/// The focus is one extra sentence in the material, not a different prompt: "keep this in
/// mind while summarizing" is exactly what a person means, and rewriting the summarizer
/// prompt per request would make summaries incomparable turn to turn.
pub async fn roll_forward_with(
    client: &dyn Provider,
    model: &str,
    previous: Option<(i32, String)>,
    newly_evicted: &[(String, String)],
    focus: Option<&str>,
) -> (i32, String) {
    let (covered_before, previous_summary) = match previous {
        Some((covered, summary)) => (covered, summary),
        None => (0, String::new()),
    };
    let covered_now = covered_before + newly_evicted.len() as i32;

    let mut material = String::new();
    if let Some(focus) = focus.map(str::trim).filter(|f| !f.is_empty()) {
        material.push_str(&format!("Keep this in mind especially: {focus}\n\n"));
    }
    if !previous_summary.trim().is_empty() {
        material.push_str("Earlier summary:\n");
        material.push_str(&previous_summary);
        material.push_str("\n\nNew turns to fold in:\n");
    }
    for (user, assistant) in newly_evicted {
        material.push_str(&format!("User: {user}\nAssistant: {assistant}\n"));
    }

    let request = CreateRequest {
        cache_prefix: None,
        model: model.to_string(),
        system: Some(SUMMARIZER_PROMPT.to_string()),
        messages: vec![Message::user_text(material.clone())],
        tools: Vec::new(),
        max_tokens: 700,
    };

    match client.create(request).await {
        Ok(response) => {
            let summary = response.text().trim().to_string();
            if summary.is_empty() {
                (
                    covered_now,
                    fallback_summary(&previous_summary, newly_evicted),
                )
            } else {
                (covered_now, summary)
            }
        }
        Err(_) => (
            covered_now,
            fallback_summary(&previous_summary, newly_evicted),
        ),
    }
}

/// The deterministic fallback: truncated concatenation. **No processing**, just "the stuff is still there".
fn fallback_summary(previous: &str, newly_evicted: &[(String, String)]) -> String {
    let mut text = String::new();
    if !previous.trim().is_empty() {
        text.push_str(previous.trim());
        text.push_str(" … ");
    }
    for (user, assistant) in newly_evicted {
        text.push_str(&format!(
            "user: {} | assistant: {} ",
            clip(user, 120),
            clip(assistant, 160)
        ));
    }
    let clipped = clip(text.trim(), FALLBACK_CHARS);
    format!("(the model summary is unavailable; here is a truncated excerpt of the conversation: {clipped})")
}

fn clip(text: &str, max: usize) -> String {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    jeeves_provider::truncate_chars(&cleaned, max)
}

/// Does this session need a summary now? Returns the turns to fold (may be empty).
///
/// Yields only when turns older than the window aren't yet covered — that's the whole
/// rolling semantics: `covered` is the waterline; process only new evictions above it.
pub fn newly_evicted(
    pairs: &[(String, String)],
    window: usize,
    covered: i32,
) -> Vec<(String, String)> {
    let evicted_len = pairs.len().saturating_sub(window);
    let covered = covered.max(0) as usize;
    if evicted_len <= covered {
        return Vec::new();
    }
    pairs[covered..evicted_len].to_vec()
}

/// How many turns fit the budget: pack from the **newest** backwards until full.
///
/// This is where "compact by token budget" lands: a smaller budget keeps fewer turns,
/// evicted ones folded into the summary by `roll_forward`. At least 1 turn is kept —
/// with nothing current, the model answers "what did you just say" from the summary.
///
/// Estimation is approximate (see `jeeves_provider::tokens`); only magnitude must be right.
pub fn turns_that_fit(pairs: &[(String, String)], budget: usize) -> usize {
    let mut used = 0usize;
    let mut kept = 0usize;
    for (user, assistant) in pairs.iter().rev() {
        let cost = jeeves_provider::tokens::estimate_text(user)
            + jeeves_provider::tokens::estimate_text(assistant)
            + 8;
        if kept > 0 && used + cost > budget {
            break;
        }
        used += cost;
        kept += 1;
    }
    kept
}

/// Splices the summary into the system prompt. Empty summary returns None; the caller skips it.
pub fn summary_section(summary: &str) -> Option<String> {
    let summary = summary.trim();
    if summary.is_empty() {
        return None;
    }
    Some(format!("\nEarlier in this conversation:\n{summary}"))
}

/// Check whether any earlier turns are ready to fold.
pub async fn due(
    chat: &jeeves_state::Chat,
    session_id: &str,
    pairs: &[(String, String)],
    window: usize,
) -> bool {
    match chat.load_rollup(session_id).await {
        Ok(previous) => {
            !newly_evicted(pairs, window, previous.map_or(0, |(covered, _)| covered)).is_empty()
        }
        Err(e) => {
            eprintln!("(jeeves) cannot check whether compaction is due: {e}");
            false
        }
    }
}

pub async fn refresh(
    chat: &jeeves_state::Chat,
    client: &dyn Provider,
    model: &str,
    session_id: &str,
    pairs: &[(String, String)],
    window: usize,
) -> Result<Option<String>> {
    Ok(
        refresh_with_status(chat, client, model, session_id, pairs, window)
            .await?
            .0,
    )
}

/// Returns the summary and whether this call folded any new turns, from one rollup snapshot.
pub async fn refresh_with_status(
    chat: &jeeves_state::Chat,
    client: &dyn Provider,
    model: &str,
    session_id: &str,
    pairs: &[(String, String)],
    window: usize,
) -> Result<(Option<String>, bool)> {
    let previous = chat.load_rollup(session_id).await?;
    let covered = previous.as_ref().map_or(0, |(covered, _)| *covered);
    let fresh = newly_evicted(pairs, window, covered);
    if fresh.is_empty() {
        return Ok((previous.map(|(_, summary)| summary), false));
    }
    let (covered_now, summary) = roll_forward(client, model, previous, &fresh).await;
    match chat.save_rollup(session_id, covered_now, &summary).await {
        Err(e) => eprintln!(
            "(jeeves) the rolling summary could not be stored (this turn carries on regardless): {e}"
        ),
        Ok(false) => {
            eprintln!("(jeeves) the rolling summary was not written: the database already has one covering more turns (concurrent compaction; the new one counts)")
        }
        Ok(true) => {}
    }
    Ok((Some(summary), true))
}
