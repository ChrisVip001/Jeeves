//! Write-tool tests: gate semantics (roots, content checks, approval), edit matching,
//! and the change-hunk rendering contract. No network, real temp dirs.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::approval::{
    ApprovalBroker, ApprovalFut, ApprovalOutcome, ApprovalRequest, RememberTarget,
};
use super::fs_write::{WritePolicy, edit, render_change, write_file};
use crate::{ToolCtx, ToolRegistry};

async fn test_ctx(approval: Option<Arc<dyn ApprovalBroker>>) -> ToolCtx {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        activate: None,
        cwd: Some(home.clone()),
        approval,
        jobs: None,
        turn_limits: std::sync::Arc::new(crate::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home,
        on_delta: None,
    }
}

fn registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    let policy = WritePolicy::default();
    reg.register(write_file(policy.clone()));
    reg.register(edit(policy));
    reg
}

struct Fake {
    answer: bool,
    seen: Mutex<Vec<ApprovalRequest>>,
}

impl Fake {
    fn answering(answer: bool) -> Arc<Self> {
        Arc::new(Self {
            answer,
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl ApprovalBroker for Fake {
    fn request(&self, request: ApprovalRequest) -> ApprovalFut {
        self.seen.lock().expect("lock").push(request);
        let answer = self.answer;
        Box::pin(async move {
            if answer {
                ApprovalOutcome::Approved
            } else {
                ApprovalOutcome::Denied
            }
        })
    }
}

#[tokio::test]
async fn write_file_creates_a_file_and_reports_it() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("notes/created.md");
    let out = reg
        .execute(
            ctx.clone(),
            "write_file",
            json!({"path": path.display().to_string(), "content": "first line\nsecond line\n"}),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
    assert!(out.contains("(new, 2 lines)"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file should exist"),
        "first line\nsecond line\n"
    );
}

#[tokio::test]
async fn writing_identical_content_skips_the_disk() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("same.md");
    let args = |content: &str| json!({"path": path.display().to_string(), "content": content});
    let first = reg
        .execute(ctx.clone(), "write_file", args("the same content\n"))
        .await;
    assert!(first.starts_with("Wrote"), "{first}");
    let again = reg
        .execute(ctx.clone(), "write_file", args("the same content\n"))
        .await;
    assert!(again.contains("content is unchanged"), "{again}");
}

#[tokio::test]
async fn a_write_outside_the_roots_is_refused_without_asking() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": "/etc/jeeves-fs-write-test-denied.txt", "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("outside the writable scope"), "{out}");
    assert!(!std::path::Path::new("/etc/jeeves-fs-write-test-denied.txt").exists());
}

#[tokio::test]
async fn no_chosen_workspace_means_no_writes_at_all() {
    let mut ctx = test_ctx(None).await;
    ctx.cwd = None;
    let reg = registry();
    let path = ctx.home.join("notes/nope.md");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(
        out.contains("No working directory has been chosen yet"),
        "{out}"
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn a_write_outside_the_chosen_workspace_is_refused_even_under_home() {
    let mut ctx = test_ctx(None).await;
    let workspace = ctx.home.join("workspace");
    std::fs::create_dir_all(&workspace).expect("create the working directory");
    ctx.cwd = Some(workspace);
    let path = ctx.home.join("memory/escape.md");
    let out = registry()
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.contains("outside the writable scope"), "{out}");
    assert!(!path.exists());
}

#[tokio::test]
async fn binary_content_is_refused() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("bin.md");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": "a\0b"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("NUL"), "{out}");
}

#[tokio::test]
async fn oversized_content_is_refused() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let big = "x".repeat(super::fs_write::MAX_WRITE_BYTES + 1);
    let path = ctx.home.join("big.txt");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": big}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("limit"), "{out}");
}

#[tokio::test]
async fn edit_replaces_the_single_match_and_shows_the_hunk() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("article.md");
    std::fs::write(&path, "alpha beta gamma\nsecond line\n").expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({
                "path": path.display().to_string(),
                "old_string": "beta",
                "new_string": "delta"
            }),
        )
        .await;
    assert!(out.starts_with("Edited"), "{out}");
    assert!(out.contains("- alpha beta gamma"), "{out}");
    assert!(out.contains("+ alpha delta gamma"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("it should be rewritten"),
        "alpha delta gamma\nsecond line\n"
    );
}

#[tokio::test]
async fn edit_refuses_zero_matches_with_a_hint() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("article.md");
    std::fs::write(&path, "alpha beta gamma\n").expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({"path": path.display().to_string(), "old_string": "zeta", "new_string": "x"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("was not found"), "{out}");
    assert!(out.contains("read_file"), "{out}");
}

#[tokio::test]
async fn edit_refuses_ambiguous_matches() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("article.md");
    std::fs::write(&path, "beta here\nbeta there\n").expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({"path": path.display().to_string(), "old_string": "beta", "new_string": "x"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("2 times"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("a refusal must not touch the file"),
        "beta here\nbeta there\n"
    );
}

#[tokio::test]
async fn edit_refuses_a_no_op_and_a_missing_file() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("article.md");
    std::fs::write(&path, "alpha beta\n").expect("place the file");
    let same = reg
        .execute(
            ctx.clone(),
            "edit",
            json!({"path": path.display().to_string(), "old_string": "beta", "new_string": "beta"}),
        )
        .await;
    assert!(same.starts_with("Error:"), "{same}");
    assert!(same.contains("the same"), "{same}");

    let missing_path = ctx.home.join("nope.md");
    let missing = reg
        .execute(
            ctx,
            "edit",
            json!({"path": missing_path.display().to_string(), "old_string": "a", "new_string": "b"}),
        )
        .await;
    assert!(missing.starts_with("Error:"), "{missing}");
    assert!(missing.contains("File not found"), "{missing}");
}

#[tokio::test]
async fn edit_refuses_a_binary_file() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("blob.bin");
    std::fs::write(&path, [0xff, 0xfe, 0x00]).expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({"path": path.display().to_string(), "old_string": "a", "new_string": "b"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("UTF-8"), "{out}");
}

#[tokio::test]
async fn on_request_mode_asks_once_per_write() {
    let yes = Fake::answering(true);
    let ctx = test_ctx(Some(yes.clone())).await;
    let mut reg = ToolRegistry::new();
    let policy = WritePolicy {
        approval: true,
        approval_timeout_secs: 5,
        extra_roots: Vec::new(),
        approved_dirs: Default::default(),
    };
    reg.register(write_file(policy.clone()));
    reg.register(edit(policy));

    let path = ctx.home.join("approved.md");
    let out = reg
        .execute(
            ctx.clone(),
            "write_file",
            json!({"path": path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
    {
        let seen = yes.seen.lock().expect("lock");
        assert_eq!(seen.len(), 1, "asked once, not zero times and not many");
        assert_eq!(seen[0].tool, "write_file");
    }

    let no = Fake::answering(false);
    let ctx2 = test_ctx(Some(no)).await;
    let denied_path = ctx2.home.join("denied.md");
    let out = reg
        .execute(
            ctx2.clone(),
            "write_file",
            json!({"path": denied_path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("The user denied this write"), "{out}");
    assert!(!denied_path.exists());
}

#[tokio::test]
async fn an_approved_directory_stops_asking() {
    let yes = Fake::answering(true);
    let ctx = test_ctx(Some(yes.clone())).await;
    let approved: Arc<Mutex<Vec<std::path::PathBuf>>> = Default::default();
    let mut reg = ToolRegistry::new();
    reg.register(write_file(WritePolicy {
        approval: true,
        approval_timeout_secs: 5,
        extra_roots: Vec::new(),
        approved_dirs: approved.clone(),
    }));

    let first = ctx.home.join("first.md");
    let out = reg
        .execute(
            ctx.clone(),
            "write_file",
            json!({"path": first.display().to_string(), "content": "1\n"}),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
    {
        let seen = yes.seen.lock().expect("lock");
        assert_eq!(seen.len(), 1, "the first time has to ask");
        assert_eq!(
            seen[0].remember,
            Some(RememberTarget::Directory(ctx.home.clone())),
            "what is remembered is the directory, not the file name"
        );
    }

    approved.lock().expect("lock").push(ctx.home.clone());
    let second = ctx.home.join("second.md");
    let out = reg
        .execute(
            ctx.clone(),
            "write_file",
            json!({"path": second.display().to_string(), "content": "2\n"}),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
    assert_eq!(
        yes.seen.lock().expect("lock").len(),
        1,
        "an approved directory must not be asked about twice"
    );
}

#[tokio::test]
async fn an_approved_directory_cannot_widen_the_boundary() {
    let ctx = test_ctx(Some(Fake::answering(true))).await;
    let outside = tempfile::tempdir().expect("another directory");
    let mut reg = ToolRegistry::new();
    let approved: Arc<Mutex<Vec<std::path::PathBuf>>> = Default::default();
    approved
        .lock()
        .expect("lock")
        .push(outside.path().to_path_buf());
    reg.register(write_file(WritePolicy {
        approval: true,
        approval_timeout_secs: 5,
        extra_roots: Vec::new(),
        approved_dirs: approved,
    }));

    let path = outside.path().join("leak.md");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": "no\n"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("writable scope"), "{out}");
    assert!(
        !path.exists(),
        "a write outside the workspace must not reach the disk"
    );
}

#[tokio::test]
async fn on_request_mode_without_a_broker_refuses() {
    let ctx = test_ctx(None).await;
    let mut reg = ToolRegistry::new();
    reg.register(write_file(WritePolicy {
        approval: true,
        approval_timeout_secs: 5,
        extra_roots: Vec::new(),
        approved_dirs: Default::default(),
    }));
    let path = ctx.home.join("x.md");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({"path": path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("nobody can answer"), "{out}");
}

#[test]
fn render_change_pins_the_hunk_format() {
    assert_eq!(render_change(None, "a\nb\n"), "(new, 2 lines)");

    let hunk = render_change(
        Some("one\ntwo\nthree\nfour\nfive\n"),
        "one\ntwo\nTWO\nfour\nfive\n",
    );
    assert!(hunk.starts_with("@@"), "{hunk}");
    assert!(hunk.contains("- three"), "{hunk}");
    assert!(hunk.contains("+ TWO"), "{hunk}");
    assert!(hunk.contains("  one"), "{hunk}");

    assert_eq!(
        render_change(Some("same\n"), "same\n"),
        "(content unchanged)"
    );
}

#[test]
fn render_change_truncates_honestly() {
    let old = "x\n".repeat(100);
    let new = "y\n".repeat(100);
    let hunk = render_change(Some(&old), &new);
    assert!(hunk.contains("diff truncated"), "{hunk}");
    assert!(hunk.contains("read_file"), "{hunk}");
}

#[tokio::test]
async fn extra_root_requires_a_selected_workspace_for_file_writes() {
    let mut ctx = test_ctx(None).await;
    ctx.cwd = None;
    let cache = tempfile::tempdir().unwrap();
    let output = cache.path().join("file.txt");
    let mut registry = ToolRegistry::new();
    registry.register(write_file(WritePolicy {
        approval: false,
        approval_timeout_secs: 5,
        extra_roots: vec![cache.path().to_owned()],
        approved_dirs: Default::default(),
    }));
    let reply = registry
        .execute(
            ctx,
            "write_file",
            json!({"path": output, "content": "text"}),
        )
        .await;
    assert!(reply.starts_with("Error:"), "{reply}");
    assert!(!output.exists());
}

#[tokio::test]
async fn edit_tolerates_crlf_and_reports_the_normalized_match() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("windows.txt");
    std::fs::write(&path, "one\r\ntwo\r\nthree\r\n").expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({"path": path.display().to_string(), "old_string": "one\ntwo", "new_string": "uno\ndos"}),
        )
        .await;
    assert!(out.starts_with("Edited"), "{out}");
    assert!(out.contains("after normalizing"), "{out}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("it should be rewritten"),
        "uno\r\ndos\r\nthree\r\n"
    );
}

#[tokio::test]
async fn edit_miss_points_at_the_closest_lines() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("lib.rs");
    let body = "fn a() {}\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\n";
    std::fs::write(&path, body).expect("place the file");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({
                "path": path.display().to_string(),
                "old_string": "fn total(items: &[u32]) -> u32 {\n  items.iter().sum()\n}",
                "new_string": "fn total() {}"
            }),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("lines 2-3"), "{out}");
    assert_eq!(std::fs::read_to_string(&path).expect("untouched"), body);
}

#[tokio::test]
async fn an_edit_on_a_file_changed_since_it_was_read_is_refused_until_read_again() {
    let ctx = test_ctx(None).await;
    let mut reg = registry();
    reg.register(crate::fs::read_file());
    let path = ctx.home.join("notes.md");
    std::fs::write(&path, "one\ntwo\n").expect("place the file");
    let read = json!({ "path": path.display().to_string() });
    reg.execute(ctx.clone(), "read_file", read.clone()).await;

    std::fs::write(&path, "one\ntwo\nthree (from the user's editor)\n").expect("change it");
    let change = json!({
        "path": path.display().to_string(),
        "old_string": "two",
        "new_string": "2"
    });
    let out = reg.execute(ctx.clone(), "edit", change.clone()).await;
    assert!(
        out.contains("changed on disk since you last read it"),
        "{out}"
    );
    assert!(
        std::fs::read_to_string(&path)
            .expect("read")
            .contains("three"),
        "the user's change must survive"
    );

    reg.execute(ctx.clone(), "read_file", read).await;
    let out = reg.execute(ctx.clone(), "edit", change).await;
    assert!(out.starts_with("Edited"), "{out}");
    let again = json!({
        "path": path.display().to_string(),
        "old_string": "one",
        "new_string": "1"
    });
    let out = reg.execute(ctx, "edit", again).await;
    assert!(
        out.starts_with("Edited"),
        "its own edit counts as seen: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        "1\n2\nthree (from the user's editor)\n"
    );
}

#[tokio::test]
async fn a_file_never_read_can_still_be_overwritten() {
    let ctx = test_ctx(None).await;
    let reg = registry();
    let path = ctx.home.join("fresh.txt");
    std::fs::write(&path, "old\n").expect("place the file");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({ "path": path.display().to_string(), "content": "new\n" }),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
}

#[tokio::test]
async fn a_stale_file_is_refused_before_anyone_is_asked_to_approve() {
    let broker = Fake::answering(true);
    let ctx = test_ctx(Some(broker.clone())).await;
    let mut reg = registry();
    reg.register(crate::fs::read_file());
    let path = ctx.home.join("draft.md");
    std::fs::write(&path, "alpha\n").expect("place the file");
    reg.execute(
        ctx.clone(),
        "read_file",
        json!({ "path": path.display().to_string() }),
    )
    .await;
    std::fs::write(&path, "alpha\nbeta from the user\n").expect("change it");
    let asked_before = broker.seen.lock().expect("lock").len();

    let out = reg
        .execute(
            ctx.clone(),
            "edit",
            json!({
                "path": path.display().to_string(),
                "old_string": "alpha",
                "new_string": "ALPHA"
            }),
        )
        .await;
    assert!(out.contains("changed on disk"), "{out}");
    let out = reg
        .execute(
            ctx,
            "write_file",
            json!({ "path": path.display().to_string(), "content": "replaced\n" }),
        )
        .await;
    assert!(out.contains("changed on disk"), "{out}");
    assert_eq!(
        broker.seen.lock().expect("lock").len(),
        asked_before,
        "no approval card for a write that will be refused"
    );
}

struct CountingPolicy(Mutex<u32>);

impl crate::permissions::ExecutionPolicy for CountingPolicy {
    fn authorize(
        &self,
        _ctx: ToolCtx,
        _name: String,
        _effect: crate::ToolEffect,
        _args: serde_json::Value,
    ) -> crate::permissions::AuthorizationFuture {
        *self.0.lock().expect("lock") += 1;
        Box::pin(async {
            Ok(crate::permissions::Authorization {
                revision: 0,
                approved: true,
            })
        })
    }

    fn validate(&self, _session_id: &str, _revision: i32) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
async fn the_session_policy_is_not_consulted_for_a_stale_write() {
    let ctx = test_ctx(None).await;
    let policy = Arc::new(CountingPolicy(Mutex::new(0)));
    let mut reg = registry();
    reg.register(crate::fs::read_file());
    reg.register(crate::patch::apply_patch(WritePolicy::default()));
    reg.set_execution_policy(policy.clone());
    let path = ctx.home.join("plan.md");
    std::fs::write(&path, "first\nsecond\n").expect("place the file");
    reg.execute(
        ctx.clone(),
        "read_file",
        json!({ "path": path.display().to_string() }),
    )
    .await;
    let consulted = *policy.0.lock().expect("lock");
    std::fs::write(&path, "first\nsecond\nthird from the user\n").expect("change it");

    let patch = format!(
        "*** Begin Patch\n*** Update File: {}\n@@\n-first\n+FIRST\n*** End Patch",
        path.display()
    );
    let out = reg
        .execute(ctx.clone(), "apply_patch", json!({ "patch": patch }))
        .await;
    assert!(out.contains("changed on disk"), "{out}");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({
                "path": path.display().to_string(),
                "old_string": "second",
                "new_string": "SECOND"
            }),
        )
        .await;
    assert!(out.contains("changed on disk"), "{out}");
    assert_eq!(
        *policy.0.lock().expect("lock"),
        consulted,
        "a refused write must never reach the approval policy"
    );
    assert!(
        std::fs::read_to_string(&path)
            .expect("read")
            .contains("third from the user")
    );
}

#[tokio::test]
async fn a_new_timestamp_on_the_same_bytes_is_not_a_change() {
    let ctx = test_ctx(None).await;
    let mut reg = registry();
    reg.register(crate::fs::read_file());
    let path = ctx.home.join("touched.md");
    std::fs::write(&path, "one\ntwo\n").expect("place the file");
    reg.execute(
        ctx.clone(),
        "read_file",
        json!({ "path": path.display().to_string() }),
    )
    .await;

    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::set_times(&path, std::fs::FileTimes::new().set_modified(later))
        .expect("touch the file");
    let out = reg
        .execute(
            ctx.clone(),
            "edit",
            json!({ "path": path.display().to_string(), "old_string": "two", "new_string": "2" }),
        )
        .await;
    assert!(out.starts_with("Edited"), "{out}");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\n2\n");
}

#[tokio::test]
async fn same_size_different_bytes_is_still_a_change() {
    let ctx = test_ctx(None).await;
    let mut reg = registry();
    reg.register(crate::fs::read_file());
    let path = ctx.home.join("swapped.md");
    std::fs::write(&path, "one\ntwo\n").expect("place the file");
    reg.execute(
        ctx.clone(),
        "read_file",
        json!({ "path": path.display().to_string() }),
    )
    .await;

    std::fs::write(&path, "one\nTWO\n").expect("same length, new text");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    std::fs::set_times(&path, std::fs::FileTimes::new().set_modified(later))
        .expect("move the timestamp");
    let out = reg
        .execute(
            ctx,
            "edit",
            json!({ "path": path.display().to_string(), "old_string": "one", "new_string": "1" }),
        )
        .await;
    assert!(
        out.contains("changed on disk since you last read it"),
        "{out}"
    );
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\nTWO\n");
}
