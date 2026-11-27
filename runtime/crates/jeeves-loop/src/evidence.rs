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

impl<'a> SourceRead<'a> {
    /// Classify a successful fetch receipt, including one recovered from trusted task state.
    pub fn fetched(receipt: Option<&'a WebReceipt>) -> Self {
        match receipt {
            Some(WebReceipt::Page {
                complete: false, ..
            }) => Self::Incomplete,
            Some(WebReceipt::Page {
                requested,
                effective: Some(effective),
                complete: true,
            }) => Self::Read {
                requested,
                effective,
            },
            _ => Self::Unbound,
        }
    }
}

impl ToolOutcome {
    pub fn source_read(&self) -> SourceRead<'_> {
        match (self.name.as_str(), self.success, self.web_receipt.as_ref()) {
            ("search_web" | "fetch_url", false, _) => SourceRead::Unavailable,
            ("search_web", true, Some(WebReceipt::SearchLeads)) => SourceRead::Lead,
            ("fetch_url", true, receipt) => SourceRead::fetched(receipt),
            ("search_web", true, _) => SourceRead::Unbound,
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
    let claims = calls.iter().flat_map(|call| {
        let SourceRead::Read {
            requested,
            effective,
        } = call.source_read()
        else {
            return Vec::new();
        };
        extract(requested, effective, &call.output)
            .into_iter()
            .map(|claim| (claim, effective))
            .collect()
    });
    match adjudicate(claims, entity, freshness, qualifies) {
        Review::Insufficient => Decision::Insufficient,
        Review::Conflict(_) => Decision::Conflict,
        Review::Confirmed((claim, source)) => Decision::Confirmed {
            identity: claim.identity,
            value: claim.value,
            source: source.to_string(),
        },
    }
}

/// Review of scoped claims supplied by a host that has already verified the reads.
/// The conflict carries the observations needed to explain the disagreement.
pub enum Review<C> {
    Insufficient,
    Confirmed(C),
    Conflict(Vec<C>),
}

pub fn adjudicate<E, T, I, V, S>(
    claims: impl IntoIterator<Item = (Claim<E, T, I, V>, S)>,
    entity: &E,
    freshness: Freshness<T>,
    qualifies: impl Fn(&V) -> bool,
) -> Review<(Claim<E, T, I, V>, S)>
where
    E: PartialEq,
    T: PartialOrd,
    I: PartialEq,
    V: PartialEq,
{
    let mut claims: Vec<_> = claims
        .into_iter()
        .filter(|(claim, _)| {
            let fresh = match &freshness {
                Freshness::Any => true,
                Freshness::Within(window) => claim
                    .recorded_at
                    .as_ref()
                    .is_some_and(|time| window.contains(time)),
            };
            &claim.entity == entity && fresh
        })
        .collect();
    let Some(index) = claims.iter().position(|(claim, _)| qualifies(&claim.value)) else {
        return Review::Insufficient;
    };
    let chosen = claims.remove(index);
    let mut conflicting = Vec::new();
    for other in claims {
        let disagrees = if other.0.identity == chosen.0.identity {
            other.0.value != chosen.0.value
        } else {
            qualifies(&other.0.value)
        };
        if disagrees
            && !conflicting
                .iter()
                .any(|(claim, _): &(Claim<E, T, I, V>, S)| {
                    claim.identity == other.0.identity && claim.value == other.0.value
                })
        {
            conflicting.push(other);
        }
    }
    if conflicting.is_empty() {
        Review::Confirmed(chosen)
    } else {
        conflicting.insert(0, chosen);
        Review::Conflict(conflicting)
    }
}

/// A host renderer receives only observed calls, never the model's proposed answer.
/// The loop suppresses model deltas and publishes only the resulting constrained reply.
pub type AnswerGate<'a> = &'a (dyn Fn(&[ToolOutcome]) -> String + Send + Sync);

#[cfg(test)]
mod tests;
