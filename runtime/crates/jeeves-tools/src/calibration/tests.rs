//! Pure functions over shapes and verdicts — no tool, no model, no store.
//!
//! `LEARNED` is process-wide, so every test that records takes `exclusive()`, which
//! serializes them and clears the table on entry.

use super::*;
use serde_json::json;

fn succeed(name: &str, args: Value) {
    observe(name, &args, true, "");
}

fn reject(name: &str, args: Value) {
    observe(name, &args, false, "invalid date format");
}

#[test]
fn a_shape_describes_the_format_and_never_the_content() {
    assert_eq!(shape(&json!("2026-11-27")), Some("YYYY-MM-DD"));
    assert_eq!(shape(&json!("11/27/2026")), Some("date with / separators"));
    assert_eq!(
        shape(&json!("3f2504e0-4f89-41d3-9a0c-0305e82c3301")),
        Some("<uuid>")
    );
    assert_eq!(
        shape(&json!("2026-11-27T10:00:00Z")),
        Some("RFC 3339 timestamp")
    );
    assert_eq!(shape(&json!("40817810")), Some("<digits>"));
    assert_eq!(shape(&json!("a, b")), Some("comma-space separated list"));
    assert_eq!(shape(&json!("a,b")), Some("comma separated list"));
    assert_eq!(shape(&json!(7)), Some("<integer>"));
    assert_eq!(shape(&json!(true)), Some("<bool>"));
    // Nothing that could carry a path, a name or prose gets a shape at all.
    assert_eq!(shape(&json!("/Users/someone/taxes.pdf")), None);
    assert_eq!(shape(&json!("Ada Lovelace")), None);
    assert_eq!(shape(&json!("x".repeat(MAX_SHAPED_CHARS + 1))), None);
    assert_eq!(shape(&json!({"nested": "value"})), None);
    assert_eq!(shape(&json!(["a"])), None);
}

#[test]
fn the_hint_names_the_accepted_format_without_quoting_any_value() {
    let _serial = exclusive();
    succeed(
        "ledger_entry",
        json!({"date": "2026-11-20", "account": "40817810"}),
    );
    reject(
        "ledger_entry",
        json!({"date": "11/27/2026", "account": "40817810"}),
    );
    let hint = hint("ledger_entry", &json!({"date": "11/27/2026"})).expect("a learned hint");
    assert!(
        hint.contains("date as YYYY-MM-DD, not date with / separators"),
        "{hint}"
    );
    // The hint is about formats, so no argument value appears in it.
    assert!(!hint.contains("11/27/2026"), "{hint}");
    assert!(!hint.contains("40817810"), "{hint}");
}

#[test]
fn nothing_is_said_without_both_an_accepted_and_a_refused_shape() {
    let _serial = exclusive();
    // A refusal alone says what failed, not what would work.
    reject("report_export", json!({"day": "11/27/2026"}));
    assert_eq!(hint("report_export", &json!({"day": "11/27/2026"})), None);
    succeed("report_export", json!({"day": "2026-11-20"}));
    assert!(hint("report_export", &json!({"day": "11/27/2026"})).is_some());
    // A call already in the accepted shape gets no hint, nor does an untracked field.
    assert_eq!(hint("report_export", &json!({"day": "2026-11-27"})), None);
    assert_eq!(hint("report_export", &json!({"other": "11/27/2026"})), None);
    assert_eq!(hint("never_called", &json!({"day": "11/27/2026"})), None);
}

#[test]
fn only_a_rejection_about_the_arguments_teaches_a_format() {
    assert!(about_arguments("invalid date format"));
    assert!(about_arguments("date must match YYYY-MM-DD"));
    assert!(about_arguments("could not parse the period"));
    // The work failing says nothing about which formats the tool takes.
    assert!(!about_arguments("the upstream service timed out"));
    assert!(!about_arguments("permission denied"));
    assert!(!about_arguments("no such account"));

    let _serial = exclusive();
    succeed("period_close", json!({"period": "2026-11-20"}));
    observe(
        "period_close",
        &json!({"period": "11/27/2026"}),
        false,
        "the upstream service timed out",
    );
    assert_eq!(
        hint("period_close", &json!({"period": "11/27/2026"})),
        None,
        "a timeout must not be recorded as a refused format"
    );
}

#[test]
fn the_newer_verdict_replaces_the_older_one() {
    let _serial = exclusive();
    succeed("invoice_find", json!({"id": "2026-11-20"}));
    reject("invoice_find", json!({"id": "40817810"}));
    assert!(hint("invoice_find", &json!({"id": "40817810"})).is_some());
    // The tool now takes digits: the earlier rejection must not keep contradicting it.
    succeed("invoice_find", json!({"id": "40817810"}));
    assert_eq!(hint("invoice_find", &json!({"id": "40817810"})), None);
}

#[test]
fn the_tables_are_bounded() {
    let _serial = exclusive();
    for index in 0..MAX_FIELDS + 4 {
        succeed("wide_tool", json!({format!("f{index}"): "2026-11-20"}));
    }
    let table = LEARNED.lock().unwrap();
    assert_eq!(table["wide_tool"].len(), MAX_FIELDS);
    drop(table);

    LEARNED.lock().unwrap().clear();
    for index in 0..MAX_TOOLS + 4 {
        succeed(&format!("tool{index}"), json!({"day": "2026-11-20"}));
    }
    assert_eq!(LEARNED.lock().unwrap().len(), MAX_TOOLS);
}
