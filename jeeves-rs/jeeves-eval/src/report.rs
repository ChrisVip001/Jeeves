//! Eval reports: the latest verdict + a permanent run history.
//!
//! `eval_report.json` shows people and CI "how the latest run went"; `eval_runs.jsonl`
//! is the append-only history — swap models, change prompts, and the score curve grows here.

use std::path::Path;

use chrono::Local;

/// Writes the latest report and appends one history entry. Write failures only warn — the ledger must not block the eval itself.
pub fn write(home: &Path, suite: &str, payload: serde_json::Value) {
    let record = serde_json::json!({
        "suite": suite,
        "result": payload,
        "ran_at": Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    });
    if let Err(e) = std::fs::create_dir_all(home) {
        eprintln!(
            "(jeeves) could not create eval report directory ({}): {e}",
            home.display()
        );
        return;
    }
    let report_path = home.join("eval_report.json");
    if let Err(e) = std::fs::write(&report_path, record.to_string()) {
        eprintln!(
            "(jeeves) could not write eval_report.json ({}): {e}",
            report_path.display()
        );
    }
    let suite_path = home.join(format!("eval_report_{suite}.json"));
    if let Err(e) = std::fs::write(&suite_path, record.to_string()) {
        eprintln!("(jeeves) could not write {}: {e}", suite_path.display());
    }
    let history = home.join("eval_runs.jsonl");
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&history)
    {
        Ok(mut file) => {
            use std::io::Write;
            if let Err(e) = writeln!(file, "{record}") {
                eprintln!("(jeeves) could not append to eval_runs.jsonl: {e}");
            }
        }
        Err(e) => eprintln!("(jeeves) could not open eval_runs.jsonl: {e}"),
    }
}
