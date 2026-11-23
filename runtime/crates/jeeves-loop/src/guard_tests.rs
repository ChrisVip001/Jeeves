//! The guard's criteria and replacement rules. All pure functions — no model, no store needed.

use serde_json::json;

use super::guard::{for_model, reminder, GuardVerdict, StallGuard};

fn feed(guard: &mut StallGuard, name: &str, args: serde_json::Value, output: &str) -> GuardVerdict {
    guard.observe(name, &args, output, true)
}

fn fail(guard: &mut StallGuard, name: &str, args: serde_json::Value) -> GuardVerdict {
    guard.observe(name, &args, &format!("Error: {args} not found"), false)
}

#[test]
fn three_identical_calls_in_a_row_trip_the_guard() {
    let mut guard = StallGuard::new();
    let args = json!({"subject": "alex"});

    assert!(
        feed(&mut guard, "memory", args.clone(), "noted").is_ok(),
        "the first time: too early"
    );
    assert!(
        feed(&mut guard, "memory", args.clone(), "noted").is_ok(),
        "the second time: may be a re-check"
    );
    let verdict = feed(&mut guard, "memory", args.clone(), "noted");
    assert_eq!(verdict, GuardVerdict::Repeated { times: 3 });
    assert_eq!(guard.hits(), 1);
    assert!(guard.last_note().is_some());

    let mut guard = StallGuard::new();
    for i in 0..3 {
        feed(&mut guard, "memory", json!({"subject": i}), "noted");
    }
    assert_eq!(guard.hits(), 0, "different arguments must not trip it");
}

#[test]
fn an_alternating_batch_is_caught_even_though_it_never_repeats_consecutively() {
    let mut guard = StallGuard::new();
    let a = json!({"q": "a"});
    let b = json!({"q": "b"});

    feed(&mut guard, "memory", a.clone(), "result A");
    feed(&mut guard, "memory", b.clone(), "result B");
    feed(&mut guard, "memory", a.clone(), "result A");
    let verdict = feed(&mut guard, "memory", b.clone(), "result B");
    assert_eq!(verdict, GuardVerdict::Cyclic { period: 2 });

    let mut guard = StallGuard::new();
    for _ in 0..2 {
        feed(&mut guard, "t", json!({"i": 1}), "r1");
        feed(&mut guard, "t", json!({"i": 2}), "r2");
        feed(&mut guard, "t", json!({"i": 3}), "r3");
    }
    assert_eq!(guard.hits(), 1, "A, B, C twice over should trip it once");
}

#[test]
fn pure_repetition_is_not_reported_as_a_cycle() {
    let mut guard = StallGuard::new();
    let args = json!({"x": 1});
    feed(&mut guard, "t", args.clone(), "the same result");
    feed(&mut guard, "t", args.clone(), "the same result");
    assert_eq!(guard.hits(), 0, "one repeat does not make a loop");
}

#[test]
fn only_long_repeated_results_are_replaced_by_a_stub() {
    let short = "a very short result";
    let long = "x".repeat(600);
    let args = json!({"path": "/tmp/a"});

    let repeated = GuardVerdict::Repeated { times: 3 };
    let text = for_model(short, "read", &args, &repeated);
    assert!(
        text.contains(short),
        "a short result must not be stubbed: {text}"
    );
    assert!(
        text.contains("[guard]"),
        "but the warning has to be added: {text}"
    );

    let twice = GuardVerdict::Repeated { times: 2 };
    let text = for_model(&long, "read", &args, &twice);
    assert!(
        text.contains("called #2 with identical arguments"),
        "{text}"
    );
    assert!(
        text.contains("/tmp/a"),
        "the stub has to preview the arguments: {text}"
    );
    assert!(
        !text.contains("xxxx"),
        "the long body should be elided: {text}"
    );
    let text = for_model(&long, "read", &args, &GuardVerdict::Repeated { times: 1 });
    assert!(
        text.contains("xxxx"),
        "the first repeat still gets the whole text: {}",
        text.len()
    );
}

#[test]
fn a_clean_run_returns_the_output_untouched() {
    let output = "all normal";
    assert_eq!(
        for_model(output, "t", &json!({}), &GuardVerdict::Ok),
        output
    );
    assert!(reminder(&GuardVerdict::Ok).is_none());
}

#[test]
fn one_tool_failing_with_different_arguments_is_flagged_but_not_a_stall() {
    let mut guard = StallGuard::new();
    assert!(fail(&mut guard, "read_file", json!({"path": "a"})).is_ok());
    assert!(fail(&mut guard, "read_file", json!({"path": "b"})).is_ok());
    let verdict = fail(&mut guard, "read_file", json!({"path": "c"}));
    assert_eq!(
        verdict,
        GuardVerdict::Failing {
            name: "read_file".into(),
            times: 3
        }
    );
    assert!(
        !verdict.is_stall(),
        "a warning, never a reason to stop the turn"
    );
    let text = for_model(
        "Error: c not found",
        "read_file",
        &json!({"path": "c"}),
        &verdict,
    );
    assert!(text.starts_with("Error: c not found"), "{text}");
    assert!(text.contains("failed 3 times in a row"), "{text}");
}

#[test]
fn a_success_or_another_tool_resets_the_failure_streak() {
    let mut guard = StallGuard::new();
    fail(&mut guard, "read_file", json!({"path": "a"}));
    fail(&mut guard, "read_file", json!({"path": "b"}));
    feed(&mut guard, "read_file", json!({"path": "c"}), "contents");
    assert!(fail(&mut guard, "read_file", json!({"path": "d"})).is_ok());
    fail(&mut guard, "grep", json!({"q": "x"}));
    assert!(
        fail(&mut guard, "read_file", json!({"path": "e"})).is_ok(),
        "streaks are per tool: two read_file failures since its last success"
    );
}

#[test]
fn waiting_on_a_running_job_is_not_a_loop() {
    let mut guard = StallGuard::new();
    let poll = json!({"action": "output", "id": "j1", "wait": true});
    for _ in 0..6 {
        assert!(feed(
            &mut guard,
            "job",
            poll.clone(),
            "job is still running (0 read so far)"
        )
        .is_ok());
    }
    for _ in 0..4 {
        assert!(feed(
            &mut guard,
            "job_output",
            json!({"id": "j1"}),
            "still running"
        )
        .is_ok());
    }
    assert_eq!(guard.hits(), 0);

    let list = json!({"action": "list"});
    feed(&mut guard, "job", list.clone(), "jobs");
    feed(&mut guard, "job", list.clone(), "jobs");
    assert_eq!(
        feed(&mut guard, "job", list, "jobs"),
        GuardVerdict::Repeated { times: 3 },
        "only the output action is a poll"
    );
}

#[test]
fn a_landed_edit_starts_the_repeat_count_over() {
    let mut guard = StallGuard::new();
    let test = json!({"command": "cargo test"});
    feed(&mut guard, "run_command", test.clone(), "1 failed");
    feed(&mut guard, "run_command", test.clone(), "1 failed");
    feed(&mut guard, "edit", json!({"path": "a.rs"}), "edited");
    assert!(feed(&mut guard, "run_command", test.clone(), "1 failed").is_ok());
    assert!(feed(&mut guard, "run_command", test.clone(), "1 failed").is_ok());
    assert_eq!(
        feed(&mut guard, "run_command", test, "1 failed"),
        GuardVerdict::Repeated { times: 3 },
        "three unchanged reruns after the edit still trip it"
    );
}
