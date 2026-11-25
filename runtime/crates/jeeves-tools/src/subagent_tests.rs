//! Subagent tool contracts: how conclusions come back, how failures are worded, how recursive delegation is structurally blocked.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::subagent::{NAME, SubagentRunner, delegate_task};
use super::{BoxFut, ToolRegistry};

#[derive(Debug, Clone)]
struct Seen {
    task: String,
    schema: Option<serde_json::Value>,
    readonly: bool,
}

struct Fake {
    answer: Result<String, String>,
    seen: Mutex<Vec<Seen>>,
}

impl Fake {
    fn ok(answer: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: Ok(answer.to_string()),
            seen: Mutex::new(Vec::new()),
        })
    }

    fn failing(why: &str) -> Arc<Self> {
        Arc::new(Self {
            answer: Err(why.to_string()),
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl SubagentRunner for Fake {
    fn run(
        &self,
        task: String,
        result_schema: Option<serde_json::Value>,
        _parent: crate::ToolCtx,
        readonly: bool,
    ) -> BoxFut {
        self.seen.lock().expect("lock").push(Seen {
            task,
            schema: result_schema,
            readonly,
        });
        let answer = self.answer.clone();
        Box::pin(async move { answer.map_err(anyhow::Error::msg) })
    }
}

async fn ctx() -> super::ToolCtx {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    super::ToolCtx {
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
    }
}

#[tokio::test]
async fn a_delegation_returns_the_summary_with_a_label() {
    let runner = Fake::ok("meeting at ten.");
    let tool = delegate_task(runner.clone());
    let out = match (tool.handler)(ctx().await, json!({"task": "check today's schedule"})).await {
        Ok(out) => out,
        Err(e) => panic!("must not fail: {e}"),
    };

    assert!(out.contains("A subagent has answered"), "{out}");
    assert!(out.contains("meeting at ten."), "{out}");
    let seen = runner.seen.lock().expect("lock");
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].task, "check today's schedule");
    assert_eq!(seen[0].schema, None, "no schema means None");
}

#[tokio::test]
async fn a_failed_delegation_is_still_text() {
    let mut registry = ToolRegistry::new();
    registry.register(delegate_task(Fake::failing("the model call failed: boom")));
    let result = registry
        .execute_result(ctx().await, NAME, json!({"task": "check it"}))
        .await;
    assert!(!result.success);
    let out = result.text;
    assert!(
        out.starts_with("Error:"),
        "the Error: prefix has to stay: {out}"
    );
    assert!(out.contains("boom"), "{out}");
}

#[test]
fn the_child_tool_table_has_no_delegate_task() {
    let mut registry = ToolRegistry::new();
    registry.register(delegate_task(Fake::ok("verdict")));
    assert!(registry.names().contains(&NAME), "the parent turn has it");

    let child = registry.without(NAME);
    assert!(
        !child.names().contains(&NAME),
        "it must not be in the subagent's table: {:?}",
        child.names()
    );
    assert!(registry.names().contains(&NAME));
    assert!(child.names().iter().all(|name| *name != NAME));
}

#[tokio::test]
async fn the_task_is_required() {
    let mut registry = ToolRegistry::new();
    registry.register(delegate_task(Fake::ok("verdict")));
    let out = registry.execute(ctx().await, NAME, json!({})).await;
    assert!(out.starts_with("Error:") && out.contains("task"), "{out}");
}

#[tokio::test]
async fn a_result_schema_is_passed_through_and_the_report_is_labelled() {
    let runner = Fake::ok("meeting at ten.");
    let tool = delegate_task(runner.clone());
    let schema = json!({"type": "object", "required": ["answer"]});

    let out = match (tool.handler)(
        ctx().await,
        json!({"task": "check it", "result_schema": schema}),
    )
    .await
    {
        Ok(out) => out,
        Err(e) => panic!("must not fail: {e}"),
    };

    assert_eq!(
        runner.seen.lock().expect("lock")[0].schema,
        Some(schema),
        "the schema goes to the runner unchanged"
    );
    assert!(
        out.contains("reported"),
        "a subagent's words have to be marked reported: {out}"
    );
    assert!(out.contains("meeting at ten."), "{out}");
}

struct Echo;

impl SubagentRunner for Echo {
    fn run(
        &self,
        task: String,
        _result_schema: Option<serde_json::Value>,
        _parent: crate::ToolCtx,
        _readonly: bool,
    ) -> BoxFut {
        Box::pin(async move { Ok(format!("verdict: {task}")) })
    }
}

#[tokio::test]
async fn a_batch_runs_every_task_and_merges_in_the_order_asked() {
    let mut reg = ToolRegistry::new();
    reg.register(delegate_task(Arc::new(Echo)));
    let out = reg
        .execute(
            ctx().await,
            NAME,
            json!({"tasks": ["alpha", "beta", "gamma"]}),
        )
        .await;

    for (index, task) in ["alpha", "beta", "gamma"].iter().enumerate() {
        let header = format!("### Subtask {}: {task}", index + 1);
        assert!(out.contains(&header), "missing {header}: {out}");
    }
    let first = out.find("### Subtask 1: alpha").expect("the first piece");
    let second = out.find("### Subtask 2: beta").expect("the second piece");
    let third = out.find("### Subtask 3: gamma").expect("the third piece");
    assert!(
        first < second && second < third,
        "the order is scrambled: {out}"
    );
    assert!(out.contains("verdict: alpha"), "{out}");
}

#[tokio::test]
async fn a_batch_beyond_the_cap_is_refused() {
    let mut reg = ToolRegistry::new();
    reg.register(delegate_task(Arc::new(Echo)));
    let out = reg
        .execute(
            ctx().await,
            NAME,
            json!({"tasks": ["1", "2", "3", "4", "5"]}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(
        out.contains("more than 4") || out.contains("dispatch at most 4"),
        "{out}"
    );
}

#[tokio::test]
async fn an_empty_batch_is_refused() {
    let mut reg = ToolRegistry::new();
    reg.register(delegate_task(Arc::new(Echo)));
    let out = reg.execute(ctx().await, NAME, json!({"tasks": []})).await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(out.contains("empty array"), "{out}");
}

#[tokio::test]
async fn only_a_batch_hands_the_child_a_readonly_table() {
    let single = Fake::ok("verdict");
    let mut reg = ToolRegistry::new();
    reg.register(delegate_task(single.clone()));
    let _ = reg
        .execute(ctx().await, NAME, json!({"task": "check one thing"}))
        .await;
    {
        let seen = single.seen.lock().expect("lock");
        assert_eq!(seen.len(), 1);
        assert!(
            !seen[0].readonly,
            "a single delegation must not be downgraded to read-only"
        );
    }

    let batch = Fake::ok("verdict");
    let mut reg = ToolRegistry::new();
    reg.register(delegate_task(batch.clone()));
    let _ = reg
        .execute(ctx().await, NAME, json!({"tasks": ["alpha", "beta"]}))
        .await;
    let seen = batch.seen.lock().expect("lock");
    assert_eq!(seen.len(), 2);
    assert!(
        seen.iter().all(|call| call.readonly),
        "every subagent in a batch has to be read-only: several loops run at once, and writes to the same directory would overwrite each other"
    );
}
