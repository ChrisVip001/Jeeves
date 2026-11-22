//! `SourceSupports`: every claim in `claims.json` points at a saved source snapshot and quotes
//! it verbatim. A model-written summary is never evidence; the snapshot bytes are.
//!
//! `claims.json` is an array of `{ claimId, text, sourceRefs: [url…], quote }`. Snapshots are
//! listed in `sources/index.jsonl` (one `{url, fetchedAt, sha256, path}` per line), written by
//! `fetch_url` inside a task.
use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claim {
    claim_id: String,
    text: String,
    #[serde(default)]
    source_refs: Vec<String>,
    #[serde(default)]
    quote: String,
}

#[derive(Deserialize)]
struct Snapshot {
    url: String,
    path: String,
}

/// Digit runs in `text` (`3.5`, `1,200`, `2026`): each must appear **exactly** in the quote.
fn numbers(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut run = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        if ch.is_ascii_digit() || (!run.is_empty() && matches!(ch, '.' | ',')) {
            run.push(ch);
        } else if !run.is_empty() {
            out.push(run.trim_end_matches(['.', ',']).to_string());
            run.clear();
        }
    }
    out
}

/// Check `claims_path` against the snapshots under `root`. `Ok` is the evidence line; `Err` names
/// the first claim that is not supported.
pub fn check(root: &Path, claims_path: &Path) -> Result<String, String> {
    let raw = std::fs::read_to_string(claims_path)
        .map_err(|e| format!("Could not read claims file: {e}"))?;
    let claims: Vec<Claim> =
        serde_json::from_str(&raw).map_err(|e| format!("claims file is not valid: {e}"))?;
    if claims.is_empty() {
        return Err("the claims file has no claims".into());
    }
    let index = std::fs::read_to_string(root.join("sources/index.jsonl"))
        .map_err(|e| format!("could not read sources/index.jsonl: {e}"))?;
    let snapshots: Vec<Snapshot> = index
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(i, line)| {
            serde_json::from_str(line)
                .map_err(|e| format!("invalid sources/index.jsonl line {}: {e}", i + 1))
        })
        .collect::<Result<_, _>>()?;
    for claim in &claims {
        let id = &claim.claim_id;
        if claim.quote.trim().is_empty() {
            return Err(format!("claim {id} has no quote"));
        }
        if claim.source_refs.is_empty() {
            return Err(format!("claim {id} cites no source"));
        }
        let mut supported = false;
        for url in &claim.source_refs {
            let snapshot = snapshots
                .iter()
                .rev()
                .find(|s| &s.url == url)
                .ok_or_else(|| format!("claim {id}: no saved snapshot for {url}"))?;
            let body_path = crate::paths::inside_workspace(&root.join(&snapshot.path), root)
                .map_err(|e| format!("claim {id}: snapshot path refused ({e})"))?;
            let body = std::fs::read_to_string(&body_path)
                .map_err(|e| format!("claim {id}: could not read the snapshot of {url}: {e}"))?;
            if body.contains(&claim.quote) {
                supported = true;
            }
        }
        if !supported {
            return Err(format!(
                "claim {id}: the quote is not in any cited snapshot"
            ));
        }
        let quoted_numbers = numbers(&claim.quote);
        for number in numbers(&claim.text) {
            if !quoted_numbers.contains(&number) {
                return Err(format!(
                    "claim {id}: the number {number} is not in its quote"
                ));
            }
        }
    }
    Ok(format!(
        "{} claims, each quoted verbatim from a saved snapshot",
        claims.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(claims: &str, body: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sources")).unwrap();
        std::fs::write(dir.path().join("sources/a.txt"), body).unwrap();
        std::fs::write(
            dir.path().join("sources/index.jsonl"),
            r#"{"url":"https://x.test/a","fetchedAt":"t","sha256":"h","path":"sources/a.txt"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("claims.json"), claims).unwrap();
        dir
    }

    fn run(dir: &tempfile::TempDir) -> Result<String, String> {
        let root = dir.path().canonicalize().unwrap();
        check(&root, &root.join("claims.json"))
    }

    const OK: &str = r#"[{"claimId":"c1","text":"Revenue rose 12.5% in 2025","sourceRefs":["https://x.test/a"],"quote":"revenue grew 12.5% during 2025"}]"#;

    #[test]
    fn a_verbatim_quote_with_matching_numbers_passes() {
        let dir = world(
            OK,
            "Last year revenue grew 12.5% during 2025, said the report.",
        );
        assert!(run(&dir).is_ok());
    }

    #[test]
    fn a_quote_not_in_the_snapshot_fails() {
        let dir = world(OK, "Revenue was flat.");
        assert!(run(&dir).unwrap_err().contains("c1"));
    }

    #[test]
    fn a_number_that_is_not_in_the_quote_fails() {
        let claims = OK.replace("rose 12.5%", "rose 13.5%");
        let dir = world(&claims, "revenue grew 12.5% during 2025");
        let err = run(&dir).unwrap_err();
        assert!(err.contains("13.5"), "{err}");
    }

    #[test]
    fn numbers_must_match_whole_values_and_an_unreadable_index_is_reported() {
        let dir = world(
            &OK.replace("rose 12.5%", "rose 2.5%"),
            "revenue grew 12.5% during 2025",
        );
        assert!(run(&dir).unwrap_err().contains("2.5"));
        std::fs::write(dir.path().join("sources/index.jsonl"), "broken").unwrap();
        assert!(run(&dir).unwrap_err().contains("index.jsonl"));
    }

    #[test]
    fn missing_source_empty_claims_and_unsaved_url_fail() {
        let dir = world("[]", "x");
        assert!(run(&dir).unwrap_err().contains("no claims"));
        let dir = world(
            r#"[{"claimId":"c","text":"t","sourceRefs":[],"quote":"x"}]"#,
            "x",
        );
        assert!(run(&dir).unwrap_err().contains("cites no source"));
        let dir = world(
            r#"[{"claimId":"c","text":"t","sourceRefs":["https://other"],"quote":"x"}]"#,
            "x",
        );
        assert!(run(&dir).unwrap_err().contains("no saved snapshot"));
    }
}

mod weekly;
pub use weekly::weekly_report;
