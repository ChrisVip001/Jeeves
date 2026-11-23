//! `jeeves-memory` — the memory agents.
//!
//! Storage lives in jeeves-state; this layer owns the **decisions**:
//!
//!   * `retrieval`      keyword lookup on every turn; there is no gate model deciding whether to look.
//!   * `consolidation`  after several turns, distills facts worth keeping long-term.
//!   * `store`          the one write entry: a fact and its page change together.
//!   * `skills`         procedural memory: SKILL.md loaded only when relevant (progressive disclosure).
//!
//! Consolidation deliberately **fails open**: if it errors, logs stay undistilled (raw records stay).

pub mod bundle;
pub mod compaction;
pub mod consolidation;
pub mod frontmatter;
pub mod install;
pub mod mirror;
pub mod skills;
pub mod store;
pub mod tidy;

#[cfg(test)]
#[path = "memory_tests.rs"]
mod memory_tests;

#[cfg(test)]
#[path = "compaction_tests.rs"]
mod compaction_tests;

#[cfg(test)]
#[path = "install_tests.rs"]
mod install_tests;

use jeeves_state::{Episodes, Facts};

/// The memory context spliced into the system prompt. Empty string = nothing found
/// this time; the caller then skips the "relevant memories" header.
///
/// The user's own standing preferences ([`Facts::personal_preferences`]) come first on every
/// turn, whatever the message says: keyword search cannot connect "what should you order for
/// me?" to "prefers green tea". Search hits follow, without repeating a preference.
pub async fn retrieve_context(
    facts: &Facts,
    episodes: &Episodes,
    query: &str,
    top_k: u32,
) -> anyhow::Result<String> {
    let preferences = facts.personal_preferences().await.unwrap_or_else(|e| {
        eprintln!("(jeeves) standing preferences unavailable this turn: {e}");
        Vec::new()
    });
    let (found, episodes) = match (
        facts.search(query, top_k).await,
        episodes.search(query, 3).await,
    ) {
        (Ok(f), Ok(e)) => (f, e),
        (Ok(f), Err(e)) => {
            eprintln!("(jeeves) episode recall unavailable: {e}");
            (f, vec![])
        }
        (Err(e), Ok(history)) => {
            eprintln!("(jeeves) fact recall unavailable: {e}");
            (vec![], history)
        }
        (Err(f), Err(e)) => anyhow::bail!("fact and episode recall unavailable: {f}; {e}"),
    };

    let mut seen = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for fact in preferences.iter().chain(&found) {
        if !seen.insert(fact.id) {
            continue;
        }
        let until = match facts.validity(fact.id).await {
            Ok((_, until)) => until,
            Err(e) => {
                eprintln!("(jeeves) could not verify recalled fact validity: {e}");
                continue;
            }
        };
        let temporal = until
            .map(|time| {
                format!(" (temporary claim, valid until {time}; not proof of current mood)")
            })
            .unwrap_or_default();
        lines.push(format!(
            "- **{}**: {}{}",
            fact.subject, fact.content, temporal
        ));
    }
    if !episodes.is_empty() {
        lines.push(
            "Historical episode summaries (summary date, not proof of current state):".into(),
        );
    }
    for episode in &episodes {
        lines.push(format!("- ({}) {}", episode.happened_at, episode.summary));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod consolidation_recovery_tests;
#[cfg(test)]
mod consolidation_replace_tests;
