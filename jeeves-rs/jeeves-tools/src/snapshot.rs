//! Source snapshots: what `fetch_url` returned inside a task, kept in the task's directory so
//! research can be checked against the bytes it actually read (`SourceSupports`).
use std::io::Write;
use std::path::Path;

/// Save `text` (what the model was shown) for `url` under `<cwd>/sources/`, and append one line
/// to `sources/index.jsonl`. Returns the path of the body relative to `cwd`.
pub fn save(cwd: &Path, url: &str, text: &str) -> std::io::Result<String> {
    let dir = cwd.join("sources");
    std::fs::create_dir_all(&dir)?;
    let sha = jeeves_state::ledger::digest_of(text.as_bytes());
    let relative = format!("sources/{}.txt", &sha[..16.min(sha.len())]);
    std::fs::write(cwd.join(&relative), text)?;
    let line = serde_json::json!({
        "url": url,
        "fetchedAt": chrono::Utc::now().to_rfc3339(),
        "sha256": sha,
        "path": relative,
    });
    let mut index = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("index.jsonl"))?;
    writeln!(index, "{line}")?;
    Ok(relative)
}

/// Search results are leads, not fetched source pages. Keep a separate manifest so a
/// model cannot satisfy sourceSupports by citing just a search snippet.
pub fn save_search(cwd: &Path, query: &str, url: &str, text: &str) -> std::io::Result<String> {
    let dir = cwd.join("sources/search");
    std::fs::create_dir_all(&dir)?;
    let sha = jeeves_state::ledger::digest_of(text.as_bytes());
    let relative = format!("sources/search/{sha}.txt");
    std::fs::write(cwd.join(&relative), text)?;
    let line = serde_json::json!({"query":query,"url":url,"fetchedAt":chrono::Utc::now().to_rfc3339(),"sha256":sha,"path":relative});
    let mut index = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("index.jsonl"))?;
    writeln!(index, "{line}")?;
    Ok(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_leads_do_not_count_as_fetched_sources() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_search(dir.path(), "query", "https://example.com/", "lead").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(path)).unwrap(),
            "lead"
        );
        assert!(!dir.path().join("sources/index.jsonl").exists());
        let line = std::fs::read_to_string(dir.path().join("sources/search/index.jsonl")).unwrap();
        let entry: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(entry["query"], "query");
        assert_eq!(entry["url"], "https://example.com/");
    }

    #[test]
    fn a_snapshot_is_written_and_indexed_once_per_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let path = save(dir.path(), "https://x.test/a", "hello page").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(&path)).unwrap(),
            "hello page"
        );
        save(dir.path(), "https://x.test/b", "other").unwrap();
        let index = std::fs::read_to_string(dir.path().join("sources/index.jsonl")).unwrap();
        let lines: Vec<serde_json::Value> = index
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["url"], "https://x.test/a");
        assert_eq!(lines[0]["path"], path);
        assert!(lines[0]["sha256"].as_str().unwrap().len() >= 16);
    }
}
