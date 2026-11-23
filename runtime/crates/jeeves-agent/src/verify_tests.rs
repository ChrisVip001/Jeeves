//! Acceptance-check tests: one question each, and the answers must be measurements.
//!
//! The two that matter most are the last two: a path outside the workspace is refused, and no
//! workspace means no path check at all — never "fall back to home and hope".

use jeeves_state::{NewStep, Steps};

use crate::verify::{Acceptance, Verifier, all_passed, parse};

async fn fixture() -> (tempfile::TempDir, Steps, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).expect("create home");
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    (dir, Steps::new(pool), "task1".to_string())
}

fn verifier(workspace: &std::path::Path, steps: &Steps, task_id: &str) -> Verifier {
    Verifier {
        run_id: "run1".into(),
        root: Some(workspace.to_path_buf()),
        steps: steps.clone(),
        task_id: task_id.to_string(),
        receipts: jeeves_state::Receipts::new(steps.pool()),
        commands: None,
    }
}

#[tokio::test]
async fn an_existing_file_passes_and_reports_its_size() {
    let (dir, steps, task) = fixture().await;
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the working directory");
    std::fs::write(workspace.join("report.md"), "verdict: ready to ship\n")
        .expect("write the file");

    let verifier = verifier(&workspace, &steps, &task);
    let results = verifier
        .run(&[Acceptance::FileExists {
            path: "report.md".to_string(),
        }])
        .await;
    assert_eq!(
        results[0].artifact.as_ref().unwrap().sha256,
        jeeves_state::ledger::digest_of(b"verdict: ready to ship\n")
    );
    assert_eq!(results.len(), 1);
    assert!(
        results[0].passed,
        "the file is there, so it should pass: {:?}",
        results[0].detail
    );
    assert!(
        results[0].detail.contains("report.md") && results[0].detail.contains('B'),
        "the passing evidence has to carry the file and its size: {}",
        results[0].detail
    );
}

#[tokio::test]
async fn a_missing_file_fails_with_the_measurement_not_a_guess() {
    let (dir, steps, task) = fixture().await;
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the working directory");

    let verifier = verifier(&workspace, &steps, &task);
    let results = verifier
        .run(&[Acceptance::FileExists {
            path: "report.md".to_string(),
        }])
        .await;
    assert!(!results[0].passed);
    assert!(
        results[0].detail.contains("does not exist"),
        "a failure has to state the measurement: {}",
        results[0].detail
    );
    assert!(!all_passed(&results));
}

#[tokio::test]
async fn file_contains_counts_a_plain_substring() {
    let (dir, steps, task) = fixture().await;
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the working directory");
    std::fs::write(
        workspace.join("report.md"),
        "verdict: A\nverdict: B\nverdict: C\n",
    )
    .expect("write the file");

    let verifier = verifier(&workspace, &steps, &task);
    let passed = verifier
        .run(&[Acceptance::FileContains {
            path: "report.md".to_string(),
            pattern: "verdict:".to_string(),
            min: 3,
        }])
        .await;
    assert!(
        passed[0].passed,
        "three occurrences should be enough: {}",
        passed[0].detail
    );

    let short = verifier
        .run(&[Acceptance::FileContains {
            path: "report.md".to_string(),
            pattern: "verdict:".to_string(),
            min: 4,
        }])
        .await;
    assert!(!short[0].passed);
    assert!(
        short[0].detail.contains("only 3 ×"),
        "it has to say how many were measured: {}",
        short[0].detail
    );
}

#[tokio::test]
async fn tools_ok_counts_successful_steps_only() {
    let (dir, steps, task) = fixture().await;
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the working directory");

    for (seq, status) in [(1, "ok"), (2, "ok"), (3, "failed")] {
        let key = format!("turn1:{seq}");
        let id = steps
            .begin(NewStep {
                task_id: &task,
                run_id: Some("run1"),
                seq,
                kind: "tool",
                tool: Some("write_file"),
                input_json: None,
                idempotency_key: Some(&key),
            })
            .await
            .expect("record one step");
        steps
            .close(id, status, None, None)
            .await
            .expect("take one step");
    }

    let verifier = verifier(&workspace, &steps, &task);
    let enough = verifier
        .run(&[Acceptance::ToolsOk {
            tool: "write_file".to_string(),
            min: 2,
        }])
        .await;
    assert!(
        enough[0].passed,
        "two successes should be enough: {}",
        enough[0].detail
    );

    let too_many = verifier
        .run(&[Acceptance::ToolsOk {
            tool: "write_file".to_string(),
            min: 3,
        }])
        .await;
    assert!(
        !too_many[0].passed,
        "only the successful ones count: a third failed try does not — {}",
        too_many[0].detail
    );
}

#[tokio::test]
async fn a_path_outside_the_workspace_is_refused() {
    let (dir, steps, task) = fixture().await;
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the working directory");
    std::fs::write(dir.path().join("outside.txt"), "classified").expect("write a file outside");

    let verifier = verifier(&workspace, &steps, &task);
    let results = verifier
        .run(&[Acceptance::FileExists {
            path: "../outside.txt".to_string(),
        }])
        .await;
    assert!(!results[0].passed, "an escape must be refused");
    assert!(
        results[0].detail.contains("outside the working directory"),
        "a refusal has to say it is outside the scope: {}",
        results[0].detail
    );
}

#[tokio::test]
async fn no_workspace_refuses_every_path_check() {
    let (_dir, steps, task) = fixture().await;
    let verifier = Verifier {
        run_id: "run1".into(),
        root: None,
        receipts: jeeves_state::Receipts::new(steps.pool()),
        commands: None,
        steps,
        task_id: task,
    };
    let results = verifier
        .run(&[Acceptance::FileExists {
            path: "state.db".to_string(),
        }])
        .await;
    assert!(
        !results[0].passed,
        "with no working directory there is no path check"
    );
    assert!(
        results[0].detail.contains("working directory"),
        "it has to state the reason: {}",
        results[0].detail
    );
}

#[test]
fn a_malformed_condition_is_dropped_with_its_reason() {
    let parsed = parse(r#"[{"kind":"fileExist","path":"a.md"}]"#);
    assert!(parsed.is_empty(), "a misspelt type is not a condition");

    let parsed = parse(r#"[{"kind":"fileContains","path":"a.md"}]"#);
    assert!(
        parsed.is_empty(),
        "a condition without a pattern is not collected"
    );

    let parsed = parse(r#"[{"kind":"toolsOk","tool":"write_file","min":2}]"#);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].label(), "write_file succeeds at least 2 times");
}

#[test]
fn labels_read_as_conditions() {
    assert_eq!(
        Acceptance::FileExists {
            path: "report.md".to_string()
        }
        .label(),
        "report.md exists"
    );
    assert_eq!(
        Acceptance::FileContains {
            path: "report.md".to_string(),
            pattern: "verdict".to_string(),
            min: 3
        }
        .label(),
        "report.md contains at least 3 × “verdict”"
    );
}

#[tokio::test]
async fn successes_from_an_older_run_cannot_satisfy_current_acceptance() {
    let (dir, steps, task) = fixture().await;
    let old = steps
        .begin(NewStep {
            task_id: &task,
            run_id: Some("older-run"),
            seq: 1,
            kind: "tool",
            tool: Some("write_file"),
            input_json: None,
            idempotency_key: Some("older:1"),
        })
        .await
        .unwrap();
    steps
        .close(old, "ok", None, Some("old output"))
        .await
        .unwrap();
    let verifier = verifier(dir.path(), &steps, &task);
    let condition = [Acceptance::ToolsOk {
        tool: "write_file".into(),
        min: 1,
    }];
    assert!(
        !verifier.run(&condition).await[0].passed,
        "an old delivery is not evidence for this execution"
    );
    let current = steps
        .begin(NewStep {
            task_id: &task,
            run_id: Some("run1"),
            seq: 1,
            kind: "tool",
            tool: Some("write_file"),
            input_json: None,
            idempotency_key: Some("current:1"),
        })
        .await
        .unwrap();
    steps
        .close(current, "ok", None, Some("new output"))
        .await
        .unwrap();
    assert!(verifier.run(&condition).await[0].passed);
}

#[test]
fn execution_parser_rejects_the_entire_invalid_contract() {
    for json in [
        "null",
        "invalid",
        r#"[{"kind":"fileExists","path":"report.md"},{"kind":"fileExist","path":"other.md"}]"#,
    ] {
        assert!(crate::verify::parse_checked(json).is_err());
    }
    assert_eq!(
        crate::verify::parse_checked(r#"[{"kind":"fileExists","path":"report.md"}]"#)
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn structured_values_cannot_pass_by_hiding_expected_numbers_in_prose() {
    let (dir, steps, task) = fixture().await;
    let verifier = verifier(dir.path(), &steps, &task);
    let conditions = crate::verify::parse_checked(
        r#"[{"kind":"jsonEquals","path":"report.json","pointer":"/total","expectedJson":"42"}]"#,
    )
    .unwrap();
    let path = dir.path().join("report.json");
    std::fs::write(&path, r#"{"total":41,"note":"The expected total was 42"}"#).unwrap();
    let failed = verifier.run(&conditions).await;
    assert!(!failed[0].passed);
    let digest = failed[0].artifact.as_ref().unwrap().sha256.clone();
    std::fs::write(&path, r#"{"total":42}"#).unwrap();
    let passed = verifier.run(&conditions).await;
    assert!(passed[0].passed, "{}", passed[0].detail);
    assert_ne!(digest, passed[0].artifact.as_ref().unwrap().sha256);
    std::fs::write(&path, r#"{"total":"42"}"#).unwrap();
    assert!(!verifier.run(&conditions).await[0].passed);
    std::fs::write(&path, "not JSON").unwrap();
    assert!(!verifier.run(&conditions).await[0].passed);
}

#[tokio::test]
async fn json_checks_distinguish_missing_null_and_escaped_paths_and_bound_input() {
    let (dir, steps, task) = fixture().await;
    let verifier = verifier(dir.path(), &steps, &task);
    let criteria =
        |pointer: &str, expected: &str| {
            crate::verify::parse_checked(&serde_json::json!([
        {"kind":"jsonEquals","path":"report.json","pointer":pointer,"expectedJson":expected}
    ]).to_string())
        };
    assert!(criteria("/bad~2escape", "42").is_err());
    assert!(criteria("not/a/pointer", "42").is_err());
    assert!(criteria("/field", "{").is_err());
    let path = dir.path().join("report.json");
    std::fs::write(&path, r#"{"a/b":{"~name":null}}"#).unwrap();
    assert!(
        verifier
            .run(&criteria("/a~1b/~0name", "null").unwrap())
            .await[0]
            .passed
    );
    assert!(!verifier.run(&criteria("/missing", "null").unwrap()).await[0].passed);
    assert!(
        verifier
            .run(&criteria("", r#"{"a/b":{"~name":null}}"#).unwrap())
            .await[0]
            .passed
    );
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    let result = verifier.run(&criteria("", "null").unwrap()).await;
    assert!(!result[0].passed);
    assert!(result[0].detail.contains("byte limit"));
}

struct FakeRunner(crate::verify::CommandOutcome);

impl crate::verify::CommandRunner for FakeRunner {
    fn run(&self, _argv: Vec<String>, _cwd: std::path::PathBuf) -> crate::verify::CommandFut {
        let outcome = self.0.clone();
        Box::pin(async move { outcome })
    }
}

fn receipt_check() -> Acceptance {
    Acceptance::ReceiptConfirmed {
        kind: "feishu_send".into(),
        ref_prefix: "om_".into(),
    }
}

#[tokio::test]
async fn a_claimed_receipt_fails_and_a_confirmed_one_passes() {
    let (dir, steps, task) = fixture().await;
    let v = verifier(dir.path(), &steps, &task);
    let none = v.run(&[receipt_check()]).await;
    assert!(!none[0].passed);
    assert!(
        none[0].detail.contains("no feishu_send receipt"),
        "{}",
        none[0].detail
    );

    let id = v
        .receipts
        .claim(&task, Some("run1"), "feishu_send", "om_123")
        .await
        .unwrap();
    let claimed = v.run(&[receipt_check()]).await;
    assert!(
        !claimed[0].passed,
        "the tool said it worked; nothing read it back"
    );
    assert!(
        claimed[0].detail.contains("only claimed"),
        "{}",
        claimed[0].detail
    );

    v.receipts
        .confirm(id, Some("evidence/om_123.json"))
        .await
        .unwrap();
    let confirmed = v.run(&[receipt_check()]).await;
    assert!(confirmed[0].passed, "{}", confirmed[0].detail);
}

#[tokio::test]
async fn a_failed_or_other_kind_receipt_does_not_count() {
    let (dir, steps, task) = fixture().await;
    let v = verifier(dir.path(), &steps, &task);
    let id = v
        .receipts
        .claim(&task, None, "feishu_send", "om_1")
        .await
        .unwrap();
    v.receipts.fail(id, None).await.unwrap();
    let other = v
        .receipts
        .claim(&task, None, "mail_send", "om_2")
        .await
        .unwrap();
    v.receipts.confirm(other, None).await.unwrap();
    let wrong_prefix = v
        .receipts
        .claim(&task, None, "feishu_send", "xx_3")
        .await
        .unwrap();
    v.receipts.confirm(wrong_prefix, None).await.unwrap();
    assert!(!v.run(&[receipt_check()]).await[0].passed);
}

fn command_check() -> Acceptance {
    Acceptance::CommandSucceeds {
        argv: vec!["true".into()],
        cwd: None,
    }
}

#[tokio::test]
async fn a_command_passes_on_exit_zero_and_fails_on_exit_one() {
    use crate::verify::CommandOutcome::Ran;
    let (dir, steps, task) = fixture().await;
    let mut v = verifier(dir.path(), &steps, &task);
    v.commands = Some(std::sync::Arc::new(FakeRunner(Ran {
        code: 0,
        output: String::new(),
    })));
    assert!(v.run(&[command_check()]).await[0].passed);
    v.commands = Some(std::sync::Arc::new(FakeRunner(Ran {
        code: 1,
        output: "boom".into(),
    })));
    let failed = v.run(&[command_check()]).await;
    assert!(!failed[0].passed);
    assert!(failed[0].detail.contains("exited 1") && failed[0].detail.contains("boom"));
}

#[tokio::test]
async fn a_skipped_command_is_not_a_pass_and_says_why() {
    use crate::verify::CommandOutcome::Skipped;
    let (dir, steps, task) = fixture().await;
    let mut v = verifier(dir.path(), &steps, &task);
    v.commands = Some(std::sync::Arc::new(FakeRunner(Skipped("exec off".into()))));
    let r = v.run(&[command_check()]).await;
    assert!(!r[0].passed);
    assert_eq!(r[0].detail, "skipped: exec off");
    v.commands = None;
    assert!(
        v.run(&[command_check()]).await[0]
            .detail
            .starts_with("skipped:")
    );
}

#[tokio::test]
async fn a_command_cwd_outside_the_root_is_refused() {
    let (dir, steps, task) = fixture().await;
    let mut v = verifier(dir.path(), &steps, &task);
    v.commands = Some(std::sync::Arc::new(FakeRunner(
        crate::verify::CommandOutcome::Ran {
            code: 0,
            output: String::new(),
        },
    )));
    let r = v
        .run(&[Acceptance::CommandSucceeds {
            argv: vec!["true".into()],
            cwd: Some("/etc".into()),
        }])
        .await;
    assert!(!r[0].passed, "{}", r[0].detail);
}

#[test]
fn malformed_new_conditions_are_errors_with_an_index() {
    let bad = r#"[{"kind":"receiptConfirmed","receiptKind":"x"}]"#;
    let error = crate::verify::parse_checked(bad).unwrap_err();
    assert!(
        error.contains("#1") && error.contains("refPrefix"),
        "{error}"
    );
    let bad = r#"[{"kind":"fileExists","path":"a"},{"kind":"commandSucceeds","argv":[]}]"#;
    let error = crate::verify::parse_checked(bad).unwrap_err();
    assert!(error.contains("#2") && error.contains("argv"), "{error}");
}
