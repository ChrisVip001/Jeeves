//! Behavior edges of `read_file` / `glob`. All temp dirs; runs on Windows too.

use super::*;
use crate::ToolRegistry;
use serde_json::json;

async fn ctx() -> ToolCtx {
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
        cwd: None,
        approval: None,
        jobs: None,
        computer_use: None,
        turn_limits: std::sync::Arc::new(crate::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home,
        on_delta: None,
        web_receipt: None,
    }
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(fs::read_file());
    registry.register(crate::search::glob_tool());
    registry
}

async fn run(registry: &ToolRegistry, ctx: &ToolCtx, name: &str, args: Value) -> String {
    registry.execute(ctx.clone(), name, args).await
}

#[tokio::test]
async fn reads_a_file_with_line_numbers_and_a_footer() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("article.md");
    std::fs::write(&file, "first line\nsecond line\nthird line\n").unwrap();

    let out = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(out.contains("first line"), "{out}");
    assert!(out.contains("[Showing lines 1–3 of 3]"), "{out}");
    assert!(
        !out.contains("Use offset"),
        "read to the end, so no continue hint should appear: {out}"
    );
}

#[tokio::test]
async fn pagination_slices_and_tells_the_model_how_to_continue() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("big.md");
    let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, body).unwrap();

    let out = run(
        &registry,
        &ctx,
        "read_file",
        json!({"path": file, "offset": 4, "limit": 3}),
    )
    .await;
    assert!(out.contains("line 4"), "{out}");
    assert!(out.contains("line 6"), "{out}");
    assert!(!out.contains("line 7"), "{out}");
    assert!(out.contains("[Showing lines 4–6 of 10]"), "{out}");
    assert!(out.contains("[Use offset=7 to continue.]"), "{out}");
}

#[tokio::test]
async fn an_offset_past_the_end_says_so() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("small.md");
    std::fs::write(&file, "only one line\n").unwrap();

    let out = run(
        &registry,
        &ctx,
        "read_file",
        json!({"path": file, "offset": 99}),
    )
    .await;
    assert!(out.contains("the file has 1 lines"), "{out}");
    assert!(out.contains("out of range"), "{out}");
}

#[tokio::test]
async fn the_byte_cap_cuts_at_a_line_boundary() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("wide.md");
    let body: String = (1..=60)
        .map(|i| format!("{:04}{}", i, "x".repeat(996)) + "\n")
        .collect();
    std::fs::write(&file, body).unwrap();

    let out = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    let shown_lines = out.lines().filter(|l| l.contains("xxxx")).count();
    assert!(
        (40..=51).contains(&shown_lines),
        "it has to cut on a line boundary, giving {shown_lines} lines"
    );
    assert!(out.contains("capped at 50000 bytes"), "{out}");
    assert!(out.contains("[Use offset="), "{out}");
    let matching: Vec<&str> = out.lines().filter(|l| l.contains("xxxx")).collect();
    let last = matching.last().copied().expect("at least one line");
    assert_eq!(
        last.chars().filter(|c| *c == 'x').count(),
        996,
        "a half line appeared: {last}"
    );
}

#[tokio::test]
async fn a_binary_file_is_refused_with_directions() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("blob.bin");
    std::fs::write(&file, [0xFF, 0x00, 0x41, 0x00]).unwrap();

    let out = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("binary"), "{out}");
}

#[tokio::test]
async fn a_missing_file_is_an_honest_error() {
    let registry = registry();
    let ctx = ctx().await;
    let out = run(
        &registry,
        &ctx,
        "read_file",
        json!({"path": ctx.home.join("nope.md")}),
    )
    .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("not found"), "{out}");
}

#[tokio::test]
async fn a_directory_is_redirected_to_glob() {
    let registry = registry();
    let ctx = ctx().await;
    let out = run(&registry, &ctx, "read_file", json!({"path": ctx.home})).await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("glob"), "{out}");
}

#[tokio::test]
async fn rereading_the_same_range_returns_a_placeholder() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("memo.md");
    std::fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();

    let first = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(first.contains("alpha"), "{first}");

    let second = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(
        second.contains("File unchanged since last read"),
        "the second time it should get a placeholder: {second}"
    );
    assert!(
        !second.contains("alpha"),
        "the stub must not repeat the content: {second}"
    );

    let third = run(
        &registry,
        &ctx,
        "read_file",
        json!({"path": file, "offset": 2}),
    )
    .await;
    assert!(third.contains("beta"), "{third}");

    std::fs::write(&file, "alpha\nbeta\ngamma\ndelta\n").unwrap();
    let fourth = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(
        fourth.contains("delta"),
        "a change means fresh content: {fourth}"
    );
}

#[tokio::test]
async fn the_read_cache_is_per_session() {
    let registry = registry();
    let ctx = ctx().await;
    let file = ctx.home.join("s.md");
    std::fs::write(&file, "one line\n").unwrap();

    let first = run(&registry, &ctx, "read_file", json!({"path": file})).await;
    assert!(first.contains("one line"), "{first}");

    let mut other = ctx.clone();
    other.session_id = "another session".to_string();
    let second = run(&registry, &other, "read_file", json!({"path": file})).await;
    assert!(
        second.contains("one line"),
        "another session reading for the first time gets the full text: {second}"
    );
}

#[test]
fn tilde_expands_against_home() {
    let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
        return;
    };
    assert_eq!(
        fs::expand_tilde("~/docs/x.md"),
        std::path::PathBuf::from(&home).join("docs/x.md"),
    );
    assert_eq!(fs::expand_tilde("~"), std::path::PathBuf::from(&home));
    assert_eq!(
        fs::expand_tilde("relative/path.md"),
        std::path::PathBuf::from("relative/path.md")
    );
    assert_eq!(
        fs::expand_tilde("/abs/x.md"),
        std::path::PathBuf::from("/abs/x.md")
    );
}

#[tokio::test]
async fn glob_without_a_pattern_puts_directories_first_with_sizes() {
    let registry = registry();
    let ctx = ctx().await;
    let dir = ctx.home.join("desk");
    std::fs::create_dir(&dir).unwrap();
    std::fs::create_dir(dir.join("subdirectory")).unwrap();
    std::fs::write(dir.join("b.txt"), "12345").unwrap();
    std::fs::write(dir.join("a.txt"), "x").unwrap();

    let out = run(&registry, &ctx, "glob", json!({"path": dir})).await;
    let (before, _) = out
        .split_once("subdirectory/")
        .expect("the subdirectory appears in the list");
    assert!(
        !before.contains(".txt"),
        "directories come before files: {out}"
    );
    assert!(out.contains("a.txt  (1 B)"), "{out}");
    assert!(out.contains("b.txt  (5 B)"), "{out}");
    assert!(out.contains("[3 entries]"), "{out}");
}

#[tokio::test]
async fn glob_without_a_pattern_caps_at_500_entries() {
    let registry = registry();
    let ctx = ctx().await;
    let dir = ctx.home.join("many");
    std::fs::create_dir(&dir).unwrap();
    for i in 0..600 {
        std::fs::write(dir.join(format!("f{i:04}.txt")), "").unwrap();
    }

    let out = run(&registry, &ctx, "glob", json!({"path": dir})).await;
    assert!(out.contains("[+100 more not shown"), "{out}");
    assert_eq!(
        out.lines().filter(|l| l.contains(".txt")).count(),
        500,
        "exactly 500 entries shown"
    );
}

#[tokio::test]
async fn glob_without_a_pattern_edge_cases_are_honest() {
    let registry = registry();
    let ctx = ctx().await;
    let empty = ctx.home.join("empty");
    std::fs::create_dir(&empty).unwrap();
    let out = run(&registry, &ctx, "glob", json!({"path": empty})).await;
    assert!(out.contains("empty directory"), "{out}");

    let out = run(
        &registry,
        &ctx,
        "glob",
        json!({"path": ctx.home.join("absent")}),
    )
    .await;
    assert!(out.starts_with("Error:"), "{out}");

    let file = ctx.home.join("is-a-file.txt");
    std::fs::write(&file, "").unwrap();
    let out = run(&registry, &ctx, "glob", json!({"path": file})).await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("read_file"), "{out}");
}

#[tokio::test]
async fn relative_paths_resolve_against_the_turn_cwd() {
    let registry = registry();
    let mut ctx = ctx().await;
    let work = tempfile::tempdir().expect("the working directory");
    std::fs::write(work.path().join("note.txt"), "from the working directory").unwrap();

    ctx.cwd = Some(work.path().to_path_buf());
    let out = run(&registry, &ctx, "read_file", json!({"path": "note.txt"})).await;
    assert!(out.contains("from the working directory"), "{out}");
}

#[tokio::test]
async fn without_a_turn_cwd_relative_paths_still_use_process_cwd() {
    let registry = registry();
    let ctx = ctx().await;
    std::fs::write(ctx.home.join("legacy.txt"), "the old behaviour").unwrap();

    let out = run(
        &registry,
        &ctx,
        "read_file",
        json!({"path": ctx.home.join("legacy.txt")}),
    )
    .await;
    assert!(out.contains("the old behaviour"), "{out}");
}

#[tokio::test]
async fn a_spill_address_resolves_against_home_not_only_the_cwd() {
    let ctx = ctx().await;
    let day = ctx.home.join("spill").join("20261112");
    std::fs::create_dir_all(&day).expect("create the spill directory");
    std::fs::write(day.join("exec-120000-1.txt"), "the full output is here\n")
        .expect("write the spill file");

    let elsewhere = tempfile::tempdir().expect("temp dir");
    let ctx = ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        cwd: Some(elsewhere.path().to_path_buf()),
        ..ctx
    };

    let out = run(
        &registry(),
        &ctx,
        "read_file",
        json!({"path": "spill/20261112/exec-120000-1.txt"}),
    )
    .await;
    assert!(out.contains("the full output is here"), "{out}");
}

#[tokio::test]
async fn a_plain_relative_path_never_falls_back_to_home() {
    let ctx = ctx().await;
    std::fs::write(
        ctx.home.join("notes.md"),
        "a file of the same name under home\n",
    )
    .expect("write the file");
    let elsewhere = tempfile::tempdir().expect("temp dir");
    let ctx = ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        cwd: Some(elsewhere.path().to_path_buf()),
        ..ctx
    };

    let out = run(&registry(), &ctx, "read_file", json!({"path": "notes.md"})).await;
    assert!(
        out.starts_with("Error"),
        "it must not be found under home: {out}"
    );
    assert!(!out.contains("a file of the same name under home"), "{out}");
}
