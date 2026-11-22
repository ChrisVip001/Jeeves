//! Mirroring facts into the plaintext bundle — one page per subject.
//!
//! The database is the index; `<home>/memory/` is the library a human opens. After every
//! distillation the two are reconciled here, with one rule that outranks convenience:
//! **a page a human edited is not overwritten.** We know the page we wrote (its digest is
//! stored on the facts), so a different digest means someone opened the file and changed
//! it — their edit wins, the mirror counts it as skipped, and stderr says which file.
//!
//! Not pruning is deliberate for now: a page whose facts were all forgotten stays until
//! the wiki layer (stage 2) learns to retire pages with their sources. A stale page is a
//! smaller problem than a page that vanishes under a human's cursor.

use anyhow::Result;
use jeeves_state::Facts;

use crate::bundle::{Bundle, Page};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorReport {
    pub written: usize,
    /// Pages left alone because a human had edited them.
    pub skipped: usize,
    /// Subjects whose facts are gone (pages left in place, see the module note).
    pub orphaned: usize,
}

/// Reconcile every subject's facts into one page.
pub async fn mirror_facts(facts: &Facts, bundle: &Bundle) -> Result<MirrorReport> {
    bundle.ensure()?;
    let mut report = MirrorReport::default();

    for (subject, count) in facts.subjects().await? {
        if count <= 0 {
            continue;
        }
        let rows = facts.for_subject(&subject).await?;
        if rows.is_empty() {
            report.orphaned += 1;
            continue;
        }

        let mut expected_digest: Option<String> = None;
        for row in &rows {
            if let Some(meta) = facts.meta(row.id).await? {
                if meta.page_path.is_some() {
                    expected_digest = meta.digest.clone();
                    break;
                }
            }
        }
        let slug = slug_of(&subject);
        if let Some(existing) = bundle.read(&slug)? {
            let untouched = expected_digest
                .as_deref()
                .map(|digest| digest == existing.digest)
                .unwrap_or(false);
            if !untouched {
                report.skipped += 1;
                eprintln!(
                    "(jeeves) memory page {} holds hand-written text beyond its list; its facts are synced but the file is left as you wrote it",
                    bundle.page_path(&slug)?.display()
                );
                continue;
            }
        }

        write_page(facts, bundle, &subject, &rows).await?;
        report.written += 1;
    }
    Ok(report)
}

/// Build the prompt catalog from eligible pages at read time, never from a stale index.
pub async fn recall_catalog(facts: &Facts, bundle: &Bundle) -> Result<String> {
    let mut out = String::from("# Memory catalog\n\n");
    for page in bundle.pages()? {
        if page_is_current(facts, &page).await? {
            out.push_str(&format!(
                "- [{}]({}.md) · {} — {}\n",
                page.title, page.slug, page.kind, page.summary
            ));
        }
    }
    Ok(out)
}

/// Pages with database sources are snapshots. Suppress the entire page when any
/// source is absent, time-bounded, or corrected; never leak an obsolete sibling claim.
pub async fn page_is_current(facts: &Facts, page: &Page) -> Result<bool> {
    let mut ids = Vec::new();
    for source in &page.sources {
        if let Some(value) = source.strip_prefix("fact:") {
            let Ok(id) = value.parse::<i64>() else {
                return Ok(false);
            };
            if !facts.is_durable(id).await? {
                return Ok(false);
            }
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(true);
    }
    let rows = facts.for_subject(&page.title).await?;
    let selected: Vec<_> = rows
        .into_iter()
        .filter(|row| ids.contains(&row.id))
        .collect();
    Ok(selected.len() == ids.len()
        && strip_trailers(&page.body).trim() == render_body_legacy(&page.title, &selected).trim())
}

fn slug_of(subject: &str) -> String {
    let mut slug: String = subject
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        return format!("subject-{:x}", md5_like(subject));
    }
    trimmed.chars().take(64).collect()
}

/// A tiny stable hash for slug fallbacks (FNV-1a): nothing here is security-relevant, and
/// pulling a hash crate in for one integer would not pay for itself.
fn md5_like(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce4_84222325;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn relative_path(bundle: &Bundle, slug: &str) -> String {
    let dir = bundle
        .root()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("memory");
    format!("{dir}/{slug}.md")
}

fn summary_of(rows: &[jeeves_state::FactRow]) -> String {
    let first = rows[0].content.trim();
    let mut summary: String = first.chars().take(60).collect();
    if rows.len() > 1 {
        summary.push_str(&format!("…({} in total)", rows.len()));
    }
    summary
}

/// The page body. Each bullet ends with an inline trailer
/// (`<!-- fact:12 kind=user source=user at=2026-11-17T09:00:00 -->`): invisible when rendered,
/// it ties the line to its row — a human who rewrites the sentence edits that fact, one who
/// deletes the line deletes it (`sync_edited_pages`) — and it carries what the index needs to
/// rebuild the row under the same id (`reindex_from_pages`).
fn render_body(subject: &str, rows: &[jeeves_state::FactRow]) -> String {
    render_with(subject, rows, true)
}

/// The body without trailers: what a page says, whichever trailer shape it was written with.
fn render_body_legacy(subject: &str, rows: &[jeeves_state::FactRow]) -> String {
    render_with(subject, rows, false)
}

fn render_with(subject: &str, rows: &[jeeves_state::FactRow], trailers: bool) -> String {
    let mut out = format!("# {subject}\n\n");
    for row in rows {
        if trailers {
            out.push_str(&format!(
                "- {} <!-- fact:{} kind={} source={} at={} -->\n",
                row.content.trim(),
                row.id,
                row.kind,
                row.source.replace(char::is_whitespace, "_"),
                row.created_at.as_deref().unwrap_or("-").replace(' ', "T"),
            ));
        } else {
            out.push_str(&format!("- {}\n", row.content.trim()));
        }
    }
    out.push_str("\n## Sources\n\n");
    for row in rows {
        out.push_str(&format!(
            "- fact:{} · {} · {}\n",
            row.id,
            row.source,
            row.created_at.as_deref().unwrap_or("-")
        ));
    }
    out
}

/// Write one subject's page from its rows and record the digest on every row.
async fn write_page(
    facts: &Facts,
    bundle: &Bundle,
    subject: &str,
    rows: &[jeeves_state::FactRow],
) -> Result<()> {
    let slug = slug_of(subject);
    let mut page = Page::new(
        slug.clone(),
        "fact",
        subject.to_string(),
        subject.to_string(),
        summary_of(rows),
        render_body(subject, rows),
    );
    page.sources = rows.iter().map(|row| format!("fact:{}", row.id)).collect();
    bundle.write(&page)?;
    for row in rows {
        facts
            .set_page(row.id, &relative_path(bundle, &slug), &page.digest)
            .await?;
    }
    Ok(())
}

/// What `sync_edited_pages` changed in the facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub updated: usize,
    pub deleted: usize,
    pub added: usize,
    /// Pages rewritten after the sync (pure lists, nothing a rewrite could lose).
    pub rewritten: usize,
}

impl SyncReport {
    pub fn changed(&self) -> bool {
        self.updated + self.deleted + self.added > 0
    }
}

/// One bullet of a page: its text and whatever its trailer says.
struct Bullet {
    text: String,
    id: Option<i64>,
    kind: Option<String>,
    source: Option<String>,
    at: Option<String>,
}

/// Split `text <!-- tag -->` into the text and the tag (when the line ends with a comment).
fn split_trailer(line: &str) -> (&str, Option<&str>) {
    let trimmed = line.trim_end();
    match trimmed.rfind("<!--") {
        Some(at) if trimmed.ends_with("-->") => (
            trimmed[..at].trim_end(),
            Some(trimmed[at + 4..trimmed.len() - 3].trim()),
        ),
        _ => (trimmed, None),
    }
}

fn strip_trailers(body: &str) -> String {
    body.lines()
        .map(|line| split_trailer(line).0)
        .collect::<Vec<_>>()
        .join("\n")
}

fn bullets(body: &str) -> Vec<Bullet> {
    let list = body.split("\n## Sources").next().unwrap_or(body);
    list.lines()
        .filter_map(|line| {
            let item = line
                .trim()
                .strip_prefix("- ")
                .or_else(|| line.trim().strip_prefix("* "))?;
            let (text, tag) = split_trailer(item);
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            let mut bullet = Bullet {
                text: text.to_string(),
                id: None,
                kind: None,
                source: None,
                at: None,
            };
            for token in tag.unwrap_or("").split_whitespace() {
                if let Some(id) = token.strip_prefix("fact:") {
                    bullet.id = id.parse().ok();
                } else if let Some((key, value)) = token.split_once('=') {
                    let value = Some(value.to_string());
                    match key {
                        "kind" => bullet.kind = value,
                        "source" => bullet.source = value,
                        "at" => bullet.at = value.filter(|v| v != "-").map(|v| v.replace('T', " ")),
                        _ => {}
                    }
                }
            }
            Some(bullet)
        })
        .collect()
}

/// A page is a pure list when every line before `## Sources` is its title, a bullet or blank.
/// Rewriting such a page from the facts loses nothing a human wrote.
fn is_pure_list(body: &str) -> bool {
    let list = body.split("\n## Sources").next().unwrap_or(body);
    list.lines().all(|line| {
        let line = line.trim();
        line.is_empty()
            || line.starts_with("# ")
            || line.starts_with("- ")
            || line.starts_with("* ")
    })
}

fn same_text(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// The file wins: fold hand edits of pages we generated back into the facts.
///
/// Only pages whose body no longer matches the digest we recorded are read, and only pages we
/// wrote (an untracked page is a human's own note and stays out of the facts). Each bullet is
/// matched to a fact by its trailer id, then by its text (pages from before trailers):
///
/// * a matched bullet whose text changed → `update` (the old wording is suppressed, so
///   consolidation will not bring it back);
/// * a fact the page listed but no bullet matches → `delete` (suppressed the same way);
/// * a bullet matching nothing → a new fact, `source=user`.
///
/// A page that is still a pure list is then rewritten from the facts (new trailers, new facts
/// since). A page with prose of its own keeps its text; its facts are synced all the same.
pub async fn sync_edited_pages(facts: &Facts, bundle: &Bundle) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    for page in bundle.pages()? {
        sync_page(facts, bundle, &page, &mut report).await?;
    }
    Ok(report)
}

/// The digest we recorded when we last wrote this page; `None` = not a page we wrote.
async fn recorded_digest(facts: &Facts, page: &Page) -> Result<Option<String>> {
    for source in &page.sources {
        let Some(id) = source.strip_prefix("fact:").and_then(|n| n.parse().ok()) else {
            continue;
        };
        if let Some(meta) = facts.meta(id).await? {
            if meta.page_path.is_some() && meta.digest.is_some() {
                return Ok(meta.digest);
            }
        }
    }
    Ok(None)
}

/// Whether we may rewrite `slug`: no page yet, or a page still exactly as we wrote it.
pub(crate) async fn owned(facts: &Facts, bundle: &Bundle, slug: &str) -> Result<bool> {
    Ok(match bundle.read(slug)? {
        None => true,
        Some(page) => recorded_digest(facts, &page).await?.as_deref() == Some(page.digest.as_str()),
    })
}

/// Rewrite (or remove, when no facts are left) one subject's page from the facts.
pub(crate) async fn refresh_subject(facts: &Facts, bundle: &Bundle, subject: &str) -> Result<()> {
    let rows = facts.for_subject(subject).await?;
    if rows.is_empty() {
        bundle.remove(&slug_of(subject))?;
    } else {
        write_page(facts, bundle, subject, &rows).await?;
    }
    Ok(())
}

pub(crate) fn slug_for(subject: &str) -> String {
    slug_of(subject)
}

/// Fold one page's hand edits into the facts (see `sync_edited_pages`).
pub(crate) async fn sync_page(
    facts: &Facts,
    bundle: &Bundle,
    page: &Page,
    report: &mut SyncReport,
) -> Result<()> {
    let listed: Vec<i64> = page
        .sources
        .iter()
        .filter_map(|source| source.strip_prefix("fact:")?.parse().ok())
        .collect();
    if listed.is_empty() {
        return Ok(());
    }
    let Some(recorded) = recorded_digest(facts, page).await? else {
        return Ok(());
    };
    if recorded == page.digest {
        return Ok(());
    }
    let subject = page.subject.clone();
    let current = facts.for_subject(&subject).await?;
    let on_page: Vec<_> = current
        .iter()
        .filter(|row| listed.contains(&row.id))
        .collect();
    let mut matched = std::collections::HashSet::new();
    let mut unmatched = Vec::new();
    for bullet in bullets(&page.body) {
        let text = bullet.text;
        let by_id = bullet.id.and_then(|id| {
            on_page
                .iter()
                .find(|row| row.id == id && !matched.contains(&row.id))
        });
        if let Some(row) = by_id {
            matched.insert(row.id);
            if !same_text(&row.content, &text) {
                match facts.update(row.id, &text).await {
                    Ok(true) => report.updated += 1,
                    Ok(false) => {}
                    Err(error) => eprintln!(
                        "(jeeves) memory page {}: could not apply the edit to fact {}: {error}",
                        page.slug, row.id
                    ),
                }
            }
            continue;
        }
        match on_page
            .iter()
            .find(|row| !matched.contains(&row.id) && same_text(&row.content, &text))
        {
            Some(row) => {
                matched.insert(row.id);
            }
            None => unmatched.push(text),
        }
    }
    for row in &on_page {
        if !matched.contains(&row.id) && facts.delete(row.id).await? {
            report.deleted += 1;
        }
    }
    for text in unmatched {
        match facts.add(&subject, &text, "user", "fact").await {
            Ok((_, true)) => report.added += 1,
            Ok((_, false)) => {}
            Err(error) => eprintln!(
                "(jeeves) memory page {}: could not add \"{text}\": {error}",
                page.slug
            ),
        }
    }
    if is_pure_list(&page.body) {
        let rows = facts.for_subject(&subject).await?;
        if rows.is_empty() {
            bundle.remove(&page.slug)?;
        } else {
            write_page(facts, bundle, &subject, &rows).await?;
        }
        report.rewritten += 1;
    }
    Ok(())
}

/// Rebuild the index from the pages: every bullet whose trailer names a fact that is no longer
/// in the table is put back under the same id, so everything hung on that id (validity,
/// observations, the memory center's links) lines up again.
///
/// Only durable facts live on pages, so this restores those; time-bounded facts, short-lived
/// states and suppressions exist only in the table. A claim a human deleted, corrected or forgot
/// is suppressed and is not restored; an id already taken by another fact is left alone.
pub async fn reindex_from_pages(facts: &Facts, bundle: &Bundle) -> Result<usize> {
    let mut restored = 0;
    for page in bundle.pages()? {
        let path = relative_path(bundle, &page.slug);
        for bullet in bullets(&page.body) {
            let Some(id) = bullet.id else { continue };
            if facts.get(id).await?.is_some() {
                continue;
            }
            let row = jeeves_state::FactRow {
                id,
                subject: page.subject.clone(),
                content: bullet.text,
                source: bullet.source.unwrap_or_else(|| "user".into()),
                kind: bullet.kind.unwrap_or_else(|| "fact".into()),
                created_at: bullet.at,
            };
            if facts.restore(&row).await? {
                facts.set_page(id, &path, &page.digest).await?;
                restored += 1;
            }
        }
    }
    Ok(restored)
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod mirror_tests;
