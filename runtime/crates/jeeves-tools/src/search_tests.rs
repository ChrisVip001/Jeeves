//! `glob` / `grep`: pattern matching, the walker's skip rules, and the output contract.
//!
//! All temp dirs; no shell, no network. The two properties worth doubting are covered here:
//! a pattern with no `/` means "this file name anywhere", and a search never walks into
//! `.git` / `node_modules` / `target` (a grep that returns forty thousand hits is worse than
//! no grep).

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
        turn_limits: std::sync::Arc::new(crate::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home,
        on_delta: None,
    }
}

async fn run(registry: &ToolRegistry, ctx: &ToolCtx, name: &str, args: Value) -> String {
    registry.execute(ctx.clone(), name, args).await
}

fn search_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(search::glob_tool());
    registry.register(search::grep_tool());
    registry
}

async fn ctx_in(dir: &std::path::Path) -> ToolCtx {
    let ctx = ctx().await;
    ToolCtx {
        cwd: Some(dir.to_path_buf()),
        ..ctx
    }
}

#[test]
fn glob_without_a_slash_matches_the_file_name_at_any_depth() {
    assert!(search::glob_match("*.rs", "a.rs"));
    assert!(search::glob_match("*.rs", "src/deep/mod.rs"));
    assert!(!search::glob_match("*.rs", "src/deep/mod.ts"));
    assert!(search::glob_match("mod.rs", "src/deep/mod.rs"));
}

#[test]
fn glob_handles_double_star_and_question_mark() {
    assert!(
        search::glob_match("**/*.rs", "a.rs"),
        "`**` matches zero segments"
    );
    assert!(search::glob_match("**/*.rs", "src/a/b.rs"));
    assert!(search::glob_match("src/**/mod.rs", "src/a/b/mod.rs"));
    assert!(search::glob_match("src/*.rs", "src/a.rs"));
    assert!(
        !search::glob_match("src/*.rs", "src/a/b.rs"),
        "`*` stays in one segment"
    );
    assert!(search::glob_match("v?.toml", "v1.toml"));
    assert!(!search::glob_match("v?.toml", "v10.toml"));
}

#[tokio::test]
async fn glob_lists_files_and_skips_dependency_trees() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("src/deep")).expect("mkdir");
    std::fs::create_dir_all(dir.path().join("node_modules/pkg")).expect("mkdir");
    std::fs::write(dir.path().join("src/deep/mod.rs"), "fn a() {}\n").expect("write");
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").expect("write");
    std::fs::write(dir.path().join("node_modules/pkg/index.ts"), "noise\n").expect("write");
    let ctx = ctx_in(dir.path()).await;

    let out = run(&search_registry(), &ctx, "glob", json!({"pattern": "*.rs"})).await;
    assert!(out.contains("src/main.rs"), "{out}");
    assert!(out.contains("src/deep/mod.rs"), "{out}");
    assert!(
        out.contains("[2 file(s)]"),
        "the count is part of the answer: {out}"
    );

    let out = run(&search_registry(), &ctx, "glob", json!({"pattern": "*.ts"})).await;
    assert!(
        out.starts_with("No file"),
        "node_modules is never walked: {out}"
    );
}

#[tokio::test]
async fn glob_says_so_when_nothing_matches() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ctx = ctx_in(dir.path()).await;
    let out = run(
        &search_registry(),
        &ctx,
        "glob",
        json!({"pattern": "**/*.zig"}),
    )
    .await;
    assert!(out.starts_with("No file under ."), "{out}");
    assert!(out.contains("skip"), "it has to say what it skipped: {out}");
}

#[tokio::test]
async fn grep_returns_path_line_and_text() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("src")).expect("mkdir");
    std::fs::write(
        dir.path().join("src/a.rs"),
        "fn first() {}\nconst MAX: u32 = 3;\nfn second() {}\n",
    )
    .expect("write");
    std::fs::write(dir.path().join("src/b.rs"), "const OTHER: u32 = 4;\n").expect("write");
    let ctx = ctx_in(dir.path()).await;

    let out = run(
        &search_registry(),
        &ctx,
        "grep",
        json!({"pattern": "const "}),
    )
    .await;
    assert!(out.contains("src/a.rs:2: const MAX: u32 = 3;"), "{out}");
    assert!(out.contains("src/b.rs:1: const OTHER: u32 = 4;"), "{out}");
    assert!(out.contains("[2 match(es)]"), "{out}");
}

#[tokio::test]
async fn grep_honours_the_glob_filter_and_ignore_case() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("a.rs"), "MaxIterations = 3\n").expect("write");
    std::fs::write(dir.path().join("a.ts"), "maxIterations = 3\n").expect("write");
    let ctx = ctx_in(dir.path()).await;

    let only_ts = run(
        &search_registry(),
        &ctx,
        "grep",
        json!({"pattern": "maxiterations", "glob": "*.ts", "ignore_case": true}),
    )
    .await;
    assert!(only_ts.contains("a.ts:1:"), "{only_ts}");
    assert!(
        !only_ts.contains("a.rs"),
        "the glob filter has to hold: {only_ts}"
    );

    let case_sensitive = run(
        &search_registry(),
        &ctx,
        "grep",
        json!({"pattern": "maxiterations"}),
    )
    .await;
    assert!(
        case_sensitive.starts_with("No match"),
        "case matters unless asked otherwise: {case_sensitive}"
    );
}

#[tokio::test]
async fn grep_skips_binary_files() {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("blob.bin"), b"needle\x00more").expect("write");
    std::fs::write(dir.path().join("notes.md"), "a needle in text\n").expect("write");
    let ctx = ctx_in(dir.path()).await;

    let out = run(
        &search_registry(),
        &ctx,
        "grep",
        json!({"pattern": "needle"}),
    )
    .await;
    assert!(!out.contains("blob.bin"), "a NUL byte means binary: {out}");
    assert!(out.contains("notes.md:1: a needle in text"), "{out}");
}

#[tokio::test]
async fn grep_stops_at_the_match_cap_and_says_so() {
    let dir = tempfile::tempdir().expect("temp dir");
    let body = "needle\n".repeat(search::SEARCH_MAX_MATCHES + 5);
    std::fs::write(dir.path().join("many.txt"), body).expect("write");
    let ctx = ctx_in(dir.path()).await;

    let out = run(
        &search_registry(),
        &ctx,
        "grep",
        json!({"pattern": "needle"}),
    )
    .await;
    assert!(
        out.contains(&format!("[{} match(es)]", search::SEARCH_MAX_MATCHES)),
        "{out}"
    );
    assert!(
        out.contains("[Stopped at 200"),
        "the cap has to be visible: {out}"
    );
}

#[tokio::test]
async fn grep_without_a_pattern_says_which_argument_is_missing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let ctx = ctx_in(dir.path()).await;
    let out = run(&search_registry(), &ctx, "grep", json!({})).await;
    assert!(out.starts_with("Error"), "{out}");
    assert!(out.contains("pattern"), "{out}");
}

#[tokio::test]
async fn code_navigation_finds_definitions_and_bounds_map() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("main.rs"),
        format!(
            "pub fn needle() {{}}\n{}fn late_symbol() {{}}\nfn _() {{}}\n",
            "fn other() {}\n".repeat(300)
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("note.txt"), "needle is mentioned here\n").unwrap();
    let ctx = ctx_in(dir.path()).await;
    let mut registry = search_registry();
    registry.register(search::find_symbol_tool());
    registry.register(search::repo_map_tool());
    let found = run(&registry, &ctx, "find_symbol", json!({"symbol":"needle"})).await;
    assert!(found.contains("main.rs:1:"), "{found}");
    assert!(!found.contains("note.txt"), "{found}");
    let late = run(
        &registry,
        &ctx,
        "find_symbol",
        json!({"symbol":"late_symbol"}),
    )
    .await;
    assert!(late.contains("main.rs:302:"), "{late}");
    let underscore = run(&registry, &ctx, "find_symbol", json!({"symbol":"_"})).await;
    assert!(underscore.contains("main.rs:303:"), "{underscore}");
    let long_name = "a".repeat(300);
    std::fs::write(
        dir.path().join("long.rs"),
        format!("fn {long_name}() {{}}\n"),
    )
    .unwrap();
    let long = run(&registry, &ctx, "find_symbol", json!({"symbol":long_name})).await;
    assert!(long.contains("long.rs:1:"), "{long}");
    let map = run(&registry, &ctx, "repo_map", json!({})).await;
    assert!(map.contains("Stopped"), "{map}");
    assert!(map.len() < 50000);
}
