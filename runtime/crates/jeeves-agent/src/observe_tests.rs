//! Observation tests: the difference between "the tool said it wrote" and "the file is there".
//!
//! Both scenarios below are the **same tool call with the same output text** — only the
//! filesystem differs. That is the point: the ledger reads the world, not the model's prose.

use crate::observe::SettledCall;
use serde_json::json;

use crate::observe::{Fact, observe};

fn round(tool: &str, args: serde_json::Value, output: &str, success: bool) -> Vec<SettledCall> {
    vec![SettledCall {
        seq: 1,
        tool: tool.to_owned(),
        input: args,
        output: output.to_owned(),
        success,
    }]
}

#[tokio::test]
async fn a_real_write_becomes_an_artifact() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("report.md"), "verdict: ready to ship\n")
        .expect("write the file");

    let messages = round(
        "write_file",
        json!({ "path": "report.md", "content": "verdict: ready to ship\n" }),
        "Error: report.md is the actual output",
        true,
    );
    let observation = observe(&messages, "turn1", Some(dir.path()));

    assert_eq!(observation.steps.len(), 1, "one tool call is one step");
    assert!(observation.steps[0].ok);
    assert_eq!(observation.steps[0].idempotency_key, "turn1:1");
    assert!(!observation.has_contradiction());
    let artifacts = observation.artifacts();
    assert_eq!(artifacts.len(), 1);
    assert!(artifacts[0].0.ends_with("report.md"));
    assert_eq!(artifacts[0].2, Some("verdict: ready to ship\n"));
    assert!(
        artifacts[0].1 > 0,
        "the size has to be measured, not assumed"
    );
}

#[tokio::test]
async fn a_write_that_left_no_file_is_a_contradiction() {
    let dir = tempfile::tempdir().expect("temp dir");

    let messages = round(
        "write_file",
        json!({ "path": "report.md", "content": "verdict: ready to ship\n" }),
        "Error: report.md is the actual output",
        true,
    );
    let observation = observe(&messages, "turn1", Some(dir.path()));

    assert!(
        observation.has_contradiction(),
        "claiming a write that never happened has to be caught"
    );
    assert!(
        observation.artifacts().is_empty(),
        "a file that does not exist is not a deliverable"
    );
    let said = observation.contradictions();
    assert_eq!(said.len(), 1);
    assert!(
        said[0].contains("report.md"),
        "it has to say which file: {}",
        said[0]
    );
    assert!(
        matches!(&observation.facts[0], Fact::Contradiction { .. }),
        "the fact has to be a contradiction, not a write"
    );
}

#[tokio::test]
async fn a_failed_write_with_no_file_is_not_a_contradiction() {
    let dir = tempfile::tempdir().expect("temp dir");
    let messages = round(
        "write_file",
        json!({ "path": "report.md", "content": "x" }),
        "permission denied",
        false,
    );
    let observation = observe(&messages, "turn1", Some(dir.path()));
    assert!(
        !observation.has_contradiction(),
        "failing and not writing is honest"
    );
    assert!(!observation.steps[0].ok);
}

#[test]
fn a_late_settlement_preserves_the_original_step_key() {
    let mut calls = round("read_file", json!({"path":"b"}), "content", true);
    calls[0].seq = 2;
    let second_key = observe(&calls, "round", None).steps[0]
        .idempotency_key
        .clone();
    calls.insert(
        0,
        round("read_file", json!({"path":"a"}), "content", true).remove(0),
    );
    let both = observe(&calls, "round", None);
    assert_eq!(second_key, "round:2");
    assert_eq!(both.steps[1].idempotency_key, second_key);
}

#[tokio::test]
async fn a_non_write_tool_never_produces_facts() {
    let dir = tempfile::tempdir().expect("temp dir");
    let observation = observe(
        &round(
            "run_command",
            json!({ "command": "ls" }),
            "a.md\nb.md",
            true,
        ),
        "turn1",
        Some(dir.path()),
    );
    assert!(
        observation.facts.is_empty(),
        "only a file write produces a file fact"
    );
    assert_eq!(observation.steps.len(), 1);
}

#[test]
fn human_receipts_do_not_count_as_observed_successful_tool_calls() {
    let observation = observe(
        &round(
            "external_write",
            json!({"value":42}),
            "Human reconciliation: external receipt tx-42 confirmed",
            false,
        ),
        "turn",
        None,
    );
    assert_eq!(observation.steps.len(), 1);
    assert!(!observation.steps[0].ok);
}

#[test]
fn a_denied_write_to_an_existing_file_cannot_publish_an_artifact() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("draft.txt"), "prior").unwrap();
    let messages = round(
        "write_file",
        json!({"path":"draft.txt","content":"other"}),
        "Denied",
        false,
    );
    assert!(
        observe(&messages, "turn", Some(dir.path()))
            .artifacts()
            .is_empty()
    );
}
