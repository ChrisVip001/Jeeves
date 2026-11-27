//! Deterministic claim review for host-selected evidence policies.
//! A receipt authenticates how the text arrived, not whether the page is true.
//! A host adapter must still bind claims to one scoped record and establish source suitability.

use std::ops::RangeInclusive;

use jeeves_tools::web::WebReceipt;

use crate::ToolOutcome;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceRead<'a> {
    NotWeb,
    Unavailable,
    Lead,
    Incomplete,
    Unbound,
    Read {
        requested: &'a str,
        effective: &'a str,
    },
}

impl ToolOutcome {
    pub fn source_read(&self) -> SourceRead<'_> {
        match (self.name.as_str(), self.success, self.web_receipt.as_ref()) {
            ("search_web" | "fetch_url", false, _) => SourceRead::Unavailable,
            ("search_web", true, Some(WebReceipt::SearchLeads)) => SourceRead::Lead,
            (
                "fetch_url",
                true,
                Some(WebReceipt::Page {
                    complete: false, ..
                }),
            ) => SourceRead::Incomplete,
            (
                "fetch_url",
                true,
                Some(WebReceipt::Page {
                    requested,
                    effective: Some(effective),
                    complete: true,
                }),
            ) => SourceRead::Read {
                requested: requested.as_str(),
                effective: effective.as_str(),
            },
            ("search_web" | "fetch_url", true, _) => SourceRead::Unbound,
            _ => SourceRead::NotWeb,
        }
    }
}

/// Use the record's own date/version, never the retrieval time, for a dated claim.
pub enum Freshness<T> {
    Any,
    Within(RangeInclusive<T>),
}

/// All fields must come from the same scoped record. `entity` is the queried scope
/// (e.g. direction, store and SKU, or product and release channel).
/// `identity` distinguishes candidates inside that scope.
pub struct Claim<E, T, I, V> {
    pub entity: E,
    pub recorded_at: Option<T>,
    pub identity: I,
    pub value: V,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision<I, V> {
    Insufficient,
    Conflict,
    Confirmed {
        identity: I,
        value: V,
        source: String,
    },
}

/// Consider only complete, bound reads. Exact entity and freshness matching come
/// before comparison. If one candidate qualifies, disagreement about *that same
/// candidate* still blocks confirmation even if the other value would not qualify.
/// Separate qualifying candidates are ambiguous. Repeated identical reads do not vote.
pub fn resolve<E, T, I, V, C>(
    calls: &[ToolOutcome],
    entity: &E,
    freshness: Freshness<T>,
    extract: impl Fn(&str, &str, &str) -> C,
    qualifies: impl Fn(&V) -> bool,
) -> Decision<I, V>
where
    E: PartialEq,
    T: PartialOrd,
    I: PartialEq,
    V: PartialEq,
    C: IntoIterator<Item = Claim<E, T, I, V>>,
{
    let mut claims = Vec::new();
    for call in calls {
        let SourceRead::Read {
            requested,
            effective,
        } = call.source_read()
        else {
            continue;
        };
        for claim in extract(requested, effective, &call.output) {
            let fresh = match &freshness {
                Freshness::Any => true,
                Freshness::Within(window) => claim
                    .recorded_at
                    .as_ref()
                    .is_some_and(|time| window.contains(time)),
            };
            if &claim.entity == entity && fresh {
                claims.push((claim, effective));
            }
        }
    }
    let Some(index) = claims.iter().position(|(claim, _)| qualifies(&claim.value)) else {
        return Decision::Insufficient;
    };
    let (chosen, source) = claims.swap_remove(index);
    if claims.iter().any(|(other, _)| {
        if other.identity == chosen.identity {
            other.value != chosen.value
        } else {
            qualifies(&other.value)
        }
    }) {
        return Decision::Conflict;
    }
    Decision::Confirmed {
        identity: chosen.identity,
        value: chosen.value,
        source: source.to_string(),
    }
}

/// A host renderer receives only observed calls, never the model's proposed answer.
/// The loop suppresses model deltas and publishes only the resulting constrained reply.
pub type AnswerGate<'a> = &'a (dyn Fn(&[ToolOutcome]) -> String + Send + Sync);

#[cfg(test)]
mod tests;
