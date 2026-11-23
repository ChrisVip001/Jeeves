//! Render excerpts verified against the owner’s named sources within an explicit time window.
use super::{Claim, numbers};
use chrono::{DateTime, Duration, Utc};
use jeeves_state::verification::FileMeasurement;
use serde::Deserialize;
use std::{collections::HashMap, path::Path};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Evidence {
    url: String,
    fetched_at: String,
    path: String,
    sha256: String,
}

fn read(root: &Path, relative: &str, limit: u64) -> Result<(String, FileMeasurement), String> {
    let path = crate::paths::inside_workspace(&root.join(relative), root)?;
    FileMeasurement::read_text_limited(&path, limit).map_err(|e| format!("{relative}: {e}"))
}

pub fn verified_summary(
    root: &Path,
    allowed: &[String],
    at: DateTime<Utc>,
    lookback_days: u32,
) -> Result<String, String> {
    if !(1..=90).contains(&lookback_days) {
        return Err("The evidence window must be between 1 and 90 days".into());
    }
    let (claims, _) = read(root, "claims.json", 64 * 1024)?;
    let claims: Vec<Claim> =
        serde_json::from_str(&claims).map_err(|e| format!("claims.json: {e}"))?;
    if claims.is_empty() || claims.len() > 20 {
        return Err("a verified summary requires 1 to 20 sourced claims".into());
    }
    let (index, _) = read(root, "sources/index.jsonl", 1024 * 1024)?;
    let mut snapshots = HashMap::new();
    let since = at - Duration::days(i64::from(lookback_days));
    for line in index.lines().filter(|line| !line.trim().is_empty()) {
        let entry: Evidence =
            serde_json::from_str(line).map_err(|e| format!("source index: {e}"))?;
        if !allowed.contains(&entry.url) {
            continue;
        }
        let fetched: DateTime<Utc> = entry
            .fetched_at
            .parse()
            .map_err(|_| "invalid source timestamp")?;
        if fetched > since && fetched <= at {
            snapshots.insert(entry.url.clone(), entry);
        }
    }
    let mut body = format!("Verified source summary · {} UTC\n", at.date_naive());
    let mut checked = HashMap::new();
    for claim in claims {
        let quoted_numbers = numbers(&claim.quote);
        if claim.quote.trim().is_empty()
            || claim.source_refs.is_empty()
            || numbers(&claim.text)
                .iter()
                .any(|n| !quoted_numbers.contains(n))
        {
            return Err(format!(
                "claim {} lacks supporting evidence",
                claim.claim_id
            ));
        }
        for url in &claim.source_refs {
            let source = snapshots.get(url).ok_or_else(|| {
                format!(
                    "claim {} has no authorized snapshot in the selected window",
                    claim.claim_id
                )
            })?;
            if !checked.contains_key(url) {
                let prefix = source.sha256.get(..16).ok_or("invalid snapshot digest")?;
                if !prefix.bytes().all(|c| c.is_ascii_hexdigit())
                    || source.path != format!("sources/{prefix}.txt")
                {
                    return Err(format!("snapshot path changed for {url}"));
                }
                let (text, measured) = read(root, &source.path, 1024 * 1024)?;
                if source.sha256 != measured.sha256 {
                    return Err(format!("snapshot digest or path changed for {url}"));
                }
                checked.insert(url.clone(), text);
            }
            if !checked[url].contains(&claim.quote) {
                return Err(format!(
                    "claim {} quote does not match {url}",
                    claim.claim_id
                ));
            }
        }
        body.push_str(&format!(
            "\n“{}”\n{}\n",
            claim.quote,
            claim.source_refs.join("\n")
        ));
        if body.len() > 12 * 1024 {
            return Err("verified summary exceeds 12 KiB".into());
        }
    }
    if allowed.iter().any(|url| !checked.contains_key(url)) {
        return Err("a named source is missing from the summary".into());
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const URL: &str = "https://fixture.test/research";
    #[test]
    fn only_recent_authorized_and_verified_excerpts_can_be_exported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let now: DateTime<Utc> = "2030-01-08T12:00:00Z".parse().unwrap();
        let quote = "12 samples";
        let hash = jeeves_state::ledger::digest_of(quote.as_bytes());
        let path = format!("sources/{}.txt", &hash[..16]);
        std::fs::create_dir(root.join("sources")).unwrap();
        std::fs::write(root.join(&path), quote).unwrap();
        std::fs::write(
            root.join("claims.json"),
            json!([{"claimId":"one","text":"12 verified","quote":quote,"sourceRefs":[URL]}])
                .to_string(),
        )
        .unwrap();
        let index = |age: i64, sha: &str| {
            std::fs::write(root.join("sources/index.jsonl"),
            json!({"url":URL,"sha256":sha,"fetchedAt":(now-Duration::days(age)).to_rfc3339(),"path":path}).to_string()).unwrap()
        };
        let allowed = [URL.to_string()];
        index(1, &hash);
        let body = verified_summary(root, &allowed, now, 7).unwrap();
        assert!(body.contains(quote) && body.contains(URL));
        assert!(!body.contains("verified"));
        assert!(verified_summary(root, &["https://other.test".into()], now, 7).is_err());
        index(7, &hash);
        assert!(verified_summary(root, &allowed, now, 7).is_err());
        index(-1, &hash);
        assert!(verified_summary(root, &allowed, now, 7).is_err());
        index(1, "forged");
        assert!(verified_summary(root, &allowed, now, 7).is_err());
    }
}
