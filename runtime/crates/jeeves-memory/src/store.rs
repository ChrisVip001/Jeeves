//! The one write entry for durable memory: change a fact and its page in the same call.
//!
//! Before this, a write landed in the facts table and reached `<home>/memory/` only when the
//! background worker next mirrored (up to a minute later), and a page a human had just edited
//! could be raced. `apply` does it in order:
//!
//! 1. fold any pending hand edit of the subject's page into the facts (the file wins);
//! 2. make the change in the facts (suppression rules unchanged);
//! 3. rewrite the subject's page from the facts — or remove it when nothing is left — unless
//!    the page holds a human's own prose, which is never overwritten.
//!
//! Consolidation still writes the table and leaves the page to the worker's mirror: that old
//! path is kept for one release as the parity baseline (see `store_tests.rs`).

use anyhow::Result;
use jeeves_state::{FactRow, Facts};

use crate::bundle::Bundle;
use crate::mirror::{SyncReport, owned, refresh_subject, slug_for, sync_page};

/// One change to durable memory.
#[derive(Debug, Clone)]
pub enum Change<'a> {
    /// Remember a fact. `source` is who said it (`user`, `mcp`, …).
    Remember {
        subject: &'a str,
        content: &'a str,
        source: &'a str,
        kind: &'a str,
    },
    /// Rewrite one fact by id.
    Update { id: i64, content: &'a str },
    /// Delete one fact by id.
    Delete { id: i64 },
    /// Forget everything about a subject — its page goes too.
    ForgetSubject { subject: &'a str },
}

/// What happened, in the shape each caller already reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Remembered {
        row: FactRow,
        is_new: bool,
    },
    /// `false` = no fact with that id.
    Updated(bool),
    Deleted(bool),
    Forgot(u64),
}

/// Apply one change to the facts and to the subject's page.
pub async fn apply(facts: &Facts, bundle: &Bundle, change: Change<'_>) -> Result<Outcome> {
    let subject = match &change {
        Change::Remember { subject, .. } | Change::ForgetSubject { subject } => {
            Some(subject.to_string())
        }
        Change::Update { id, .. } | Change::Delete { id } => {
            facts.get(*id).await?.map(|row| row.subject)
        }
    };
    let slug = subject.as_deref().map(slug_for);
    if let Some(slug) = &slug
        && let Some(page) = bundle.read(slug)?
    {
        sync_page(facts, bundle, &page, &mut SyncReport::default()).await?;
    }
    let ours = match &slug {
        Some(slug) => owned(facts, bundle, slug).await?,
        None => false,
    };
    let outcome = match change {
        Change::Remember {
            subject,
            content,
            source,
            kind,
        } => {
            let (row, is_new) = facts.add(subject, content, source, kind).await?;
            Outcome::Remembered { row, is_new }
        }
        Change::Update { id, content } => Outcome::Updated(facts.update(id, content).await?),
        Change::Delete { id } => Outcome::Deleted(facts.delete(id).await?),
        Change::ForgetSubject { subject } => Outcome::Forgot(facts.forget_subject(subject).await?),
    };
    if let (true, Some(subject)) = (ours, subject) {
        bundle.ensure()?;
        refresh_subject(facts, bundle, &subject).await?;
    }
    Ok(outcome)
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
