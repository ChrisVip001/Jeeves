//! Ledger writer tests for entry order and spill files.

use jeeves_provider::{ContentBlock, Message, Role};

use crate::ledger_writer::{entries_for_turn, LedgerSink};

fn kinds(entries: &[jeeves_state::NewEntry]) -> Vec<&'static str> {
    entries.iter().map(|entry| entry.kind).collect()
}

#[test]
fn a_turn_becomes_ordered_rows() {
    let home = tempfile::tempdir().expect("temporary directory");
    let sink = LedgerSink::new(home.path(), 32_768);
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "take a look at this error".to_string(),
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "read the log first".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call-1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({"path": "app.log"}),
                    extra: None,
                },
            ],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".to_string(),
                content: "no such file".to_string(),
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "the log does not exist".to_string(),
            }],
        },
    ];

    let entries = entries_for_turn(
        &sink,
        "s1",
        "t1",
        "take a look at this error",
        &messages,
        "the log does not exist",
    );
    assert_eq!(
        kinds(&entries),
        vec!["user", "assistant", "tool_use", "tool_result", "assistant"],
        "the request is recorded once (the copy inside messages is skipped)"
    );
    assert!(
        entries[0].payload.contains("take a look at this error"),
        "small content is inlined directly: {}",
        entries[0].payload
    );
    assert!(
        entries[2].payload.contains(r#""name":"read_file""#),
        "a tool call has to carry its name and arguments: {}",
        entries[2].payload
    );
    assert!(
        entries[3].payload.contains("no such file"),
        "a tool result has to carry its text: {}",
        entries[3].payload
    );
}

#[test]
fn a_large_payload_spills_with_a_digest() {
    let home = tempfile::tempdir().expect("temporary directory");
    let sink = LedgerSink::new(home.path(), 64);
    let big = "log line\n".repeat(200);
    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "look at the log".to_string(),
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-9".to_string(),
                content: big.clone(),
            }],
        },
    ];

    let entries = entries_for_turn(&sink, "s1", "t1", "look at the log", &messages, "");
    let spilled = entries
        .iter()
        .find(|entry| entry.kind == "tool_result")
        .expect("there is a tool result");
    assert!(
        spilled.payload.len() < big.len() / 2,
        "the row holds a pointer"
    );
    let record: serde_json::Value =
        serde_json::from_str(&spilled.payload).expect("parse spill pointer");
    let pointer = record["ptr"].as_str().expect("spill path");
    assert!(pointer.starts_with("ledger/s1/"));

    let bytes = std::fs::read(home.path().join(pointer)).expect("read the written spill");
    let content: serde_json::Value = serde_json::from_slice(&bytes).expect("parse spill content");
    assert_eq!(
        content,
        serde_json::json!({"toolUseId": "call-9", "content": big})
    );
    assert_eq!(record["bytes"], bytes.len());
    assert_eq!(record["digest"], jeeves_state::ledger::digest_of(&bytes));
}
