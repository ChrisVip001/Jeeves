//! trace / usage persistence — the LLM-Ops ledgers.
//!
//! `<home>/traces/<date>.jsonl`: one line per turn — what the user said, how it was answered,
//! how many tokens were spent. **Always on** — there is no config switch; observability is not
//! optional. Token accounting per provider/model lives in state.db (`Usage`), not in a file.
//!
//! Append-only single-line writes. A write failure only shouts to stderr —
//! a broken ledger must not drag the conversation down, but it must not go silent either.

use std::io::Write;
use std::path::Path;

use chrono::Local;

/// Append one JSON line. Create the directory if missing; report write failures to stderr, never panic.
fn append_jsonl(path: &Path, line: &str) {
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "(jeeves) the spill directory could not be created ({}): {e}",
                parent.display()
            );
            return;
        }
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(format!("{line}\n").as_bytes()) {
                eprintln!(
                    "(jeeves) writing the ledger failed ({}): {e}",
                    path.display()
                );
            }
        }
        Err(e) => eprintln!(
            "(jeeves) the ledger could not be opened ({}): {e}",
            path.display()
        ),
    }
}

/// The full trace of one turn. `meta` is the protocol's TurnMeta (gate/graph/tools/model all inside),
/// wrapped with this turn's input and output — that one line is enough to trace "why it answered that way then".
pub fn record_turn(home: &Path, record: &serde_json::Value) {
    let date = Local::now().format("%Y-%m-%d");
    append_jsonl(
        &home.join("traces").join(format!("{date}.jsonl")),
        &record.to_string(),
    );
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    #[test]
    fn lines_land_as_jsonl_and_survive_failures() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let home = dir.path();

        record_turn(home, &serde_json::json!({"turnId": "t1"}));
        record_turn(home, &serde_json::json!({"turnId": "t2"}));

        let date = Local::now().format("%Y-%m-%d").to_string();
        let trace = std::fs::read_to_string(home.join("traces").join(format!("{date}.jsonl")))
            .expect("the trace file");
        let lines: Vec<&str> = trace.lines().collect();
        assert_eq!(lines.len(), 2, "two turns make two lines: {trace}");
        for line in &lines {
            let value: serde_json::Value =
                serde_json::from_str(line).expect("every line is valid JSON");
            assert!(value.get("turnId").is_some());
        }
    }

    #[test]
    fn a_bad_home_warns_instead_of_panicking() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "not a directory").unwrap();
        record_turn(&blocker, &serde_json::json!({"turnId": "t"}));
    }
}
