//! Select application policy without teaching the runtime or loop business intents.

use chrono::NaiveDate;
use jeeves_loop::{ToolOutcome, WebPreflight};
use jeeves_tools::ToolRegistry;

use super::schedule_lookup;

type ConstrainedReply = Box<dyn Fn(&[ToolOutcome]) -> String + Send + Sync>;

#[derive(Default)]
pub(super) struct SearchPolicy {
    pub preflight: Option<WebPreflight>,
    pub prompt: &'static str,
    pub answer: Option<ConstrainedReply>,
}

pub(super) fn select(
    eligible: bool,
    message: &str,
    today: NaiveDate,
    tools: &ToolRegistry,
) -> SearchPolicy {
    let Some(lookup) = eligible
        .then(|| schedule_lookup::request(message, today))
        .flatten()
    else {
        return SearchPolicy::default();
    };
    let preflight = lookup.plan(
        message,
        tools.get("search_web").is_some(),
        tools.get("fetch_url").is_some(),
    );
    SearchPolicy {
        prompt: if preflight.is_some() {
            schedule_lookup::EVIDENCE_NOTE
        } else {
            ""
        },
        preflight,
        answer: Some(Box::new(move |calls| lookup.evidence_reply(calls))),
    }
}
