//! Tool tests: no network, real temp databases, verifying behavior and output wording.

use std::sync::Arc;

use serde_json::json;

use crate::{handlers, Tool, ToolCtx, ToolEffect, ToolRegistry};

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

#[tokio::test]
async fn default_registry_has_the_expected_tools() {
    let registry = handlers::build_default();
    assert_eq!(
        registry.names(),
        vec![
            "cancel_schedule",
            "fetch_url",
            "find_symbol",
            "find_tools",
            "glob",
            "grep",
            "list_schedules",
            "memory",
            "read_file",
            "read_page",
            "repo_map",
            "schedule_job",
            "search_web",
            "send_message"
        ]
    );
}

#[test]
fn an_undeclared_effect_is_a_write_and_a_read_must_be_declared() {
    let tool = |name: &str| Tool {
        name: name.into(),
        description: "fixture".into(),
        input_schema: json!({"type":"object"}),
        handler: Arc::new(|_, _| Box::pin(async { Ok(String::new()) })),
    };
    let mut registry = ToolRegistry::new();
    registry.register(tool("undeclared"));
    registry.register_with_effect(tool("calendar_list"), ToolEffect::Read);
    assert_eq!(registry.effect("undeclared"), ToolEffect::Write);
    assert_eq!(registry.effect("missing"), ToolEffect::Write);
    assert_eq!(registry.effect("calendar_list"), ToolEffect::Read);
}

#[tokio::test]
async fn save_note_then_search_memory_round_trips() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
        )
        .await;
    assert!(
        out.contains("state.db"),
        "the output never says where it landed: {out}"
    );
    assert!(
        out.contains("alex"),
        "the output does not repeat the content: {out}"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "search", "query": "morning"}),
        )
        .await;
    assert!(
        out.contains("likes morning meetings"),
        "it cannot be searched back: {out}"
    );
}

#[tokio::test]
async fn forget_note_reports_when_nothing_matched() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "forget", "subject": "ghost"}),
        )
        .await;
    assert!(
        out.contains("no memory about"),
        "deleting 0 should still be said out loud: {out}"
    );

    registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        )
        .await;
    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "forget", "subject": "a"}),
        )
        .await;
    assert!(
        out.contains("Forgot 1"),
        "it should report how many it deleted: {out}"
    );
}

#[tokio::test]
async fn unknown_tool_and_bad_args_become_text_not_errors() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry.execute(ctx.clone(), "nope", json!({})).await;
    assert!(out.starts_with("Error:"), "an unknown tool: {out}");
    assert!(
        out.contains("memory"),
        "it should list the tools that do exist: {out}"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "alex"}),
        )
        .await;
    assert!(
        out.starts_with("Error:") && out.contains("content"),
        "a missing argument: {out}"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": 42, "content": "x"}),
        )
        .await;
    assert!(
        out.starts_with("Error:") && out.contains("subject") && out.contains("schema"),
        "the wrong type: {out}"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "  ", "content": "x"}),
        )
        .await;
    assert!(
        out.starts_with("Error:") && out.contains("must not be empty"),
        "an empty string: {out}"
    );
}

#[tokio::test]
async fn list_memory_handles_an_empty_store() {
    let registry = handlers::build_default();
    let out = registry
        .execute(ctx().await, "memory", json!({"action": "list"}))
        .await;
    assert!(
        out.contains("empty"),
        "an empty memory needs a guiding sentence: {out}"
    );
}

#[tokio::test]
async fn explicit_memory_search_reports_an_unavailable_store_as_tool_text() {
    let registry = handlers::build_default();
    let ctx = ctx().await;
    ctx.chat.pool().close().await;
    let out = registry
        .execute(ctx, "memory", json!({"action":"search","query":"meeting"}))
        .await;
    assert!(out.starts_with("Error:"), "{out}");
}

#[tokio::test]
async fn search_memory_with_junk_query_does_not_error() {
    let registry = handlers::build_default();
    let ctx = ctx().await;
    registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        )
        .await;
    for junk in ["???", "a:b", "\"", "*", "??? ..."] {
        let out = registry
            .execute(
                ctx.clone(),
                "memory",
                json!({"action": "search", "query": junk}),
            )
            .await;
        assert!(out.contains("nothing in memory about"), "{junk:?} → {out}");
    }
    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "search", "query": ""}),
        )
        .await;
    assert!(
        out.starts_with("Error:") && out.contains("must not be empty"),
        "{out}"
    );
}

#[tokio::test]
async fn send_message_writes_a_draft_and_never_sends() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "send_message",
            json!({"to": "Alex <alex@example.com>", "body": "see you Thursday"}),
        )
        .await;
    assert!(
        out.contains("Nothing was sent"),
        "it has to say outright that nothing was sent: {out}"
    );

    let dir = ctx.home.join("outbox");
    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("the outbox directory")
        .map(|e| e.expect("a directory entry"))
        .collect();
    assert_eq!(entries.len(), 1);
    let content = std::fs::read_to_string(entries.remove(0).path()).expect("the draft");
    assert!(
        content.starts_with("To: Alex <alex@example.com>\n\nsee you Thursday\n"),
        "{content}"
    );
}

#[tokio::test]
async fn manage_memory_updates_and_deletes_by_id() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes coffee"}),
        )
        .await;
    let fact = &ctx
        .facts
        .search("coffee", 1)
        .await
        .expect("query the store")[0];

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "update", "id": fact.id, "content": "likes tea"}),
        )
        .await;
    assert!(out.contains("now reads"), "{out}");
    assert!(
        ctx.facts.search("tea", 4).await.expect("query the store")[0]
            .content
            .contains("tea"),
        "the body really has to change"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "delete", "id": fact.id}),
        )
        .await;
    assert!(out.contains("Deleted"), "{out}");
    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "delete", "id": fact.id}),
        )
        .await;
    assert!(
        out.contains("with id"),
        "deleting twice has to be said honestly: {out}"
    );

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "nuke_everything", "id": 1}),
        )
        .await;
    assert!(out.starts_with("Error:") && out.contains("action"), "{out}");
}

#[tokio::test]
async fn create_skill_writes_a_valid_skill_md_and_refuses_collisions() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({
                "action": "create_skill", "name": "Weekly Review",
                "description": "summarize the week every Monday morning",
                "body": "1. Pull episodes.\n2. Draft three bullets."
            }),
        )
        .await;
    assert!(out.contains("created"), "{out}");

    let skill = std::fs::read_to_string(
        ctx.home
            .join("skills")
            .join("weekly-review")
            .join("SKILL.md"),
    )
    .expect("SKILL.md");
    assert!(skill.starts_with("---\nname: weekly-review\n"), "{skill}");
    assert!(jeeves_memory::skills::parse_skill_text(&skill).is_some());

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "create_skill", "name": "weekly-review", "description": "x", "body": "y"}),
        )
        .await;
    assert!(out.contains("already exists"), "{out}");

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "create_skill", "name": "../evil", "description": "x", "body": "y"}),
        )
        .await;
    assert!(
        out.contains("slug"),
        "a path escape has to be stopped: {out}"
    );
    assert!(
        !ctx.home.join("evil").exists()
            && !ctx
                .home
                .parent()
                .map(|p| p.join("evil"))
                .is_some_and(|p| p.exists()),
        "nothing outside the directory tree may be written"
    );
}

#[tokio::test]
async fn a_wrong_typed_argument_is_caught_before_the_handler_runs() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": 123, "content": "x"}),
        )
        .await;

    assert!(
        out.starts_with("Error:"),
        "the Error: prefix has to stay: {out}"
    );
    assert!(out.contains("schema"), "{out}");
    assert!(out.contains("subject"), "it has to name the field: {out}");
    assert!(
        ctx.facts
            .recent(10, 0)
            .await
            .expect("query the store")
            .is_empty(),
        "a failed validation must not reach the store"
    );
}

#[tokio::test]
async fn a_missing_required_argument_is_caught_by_the_schema() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "alex"}),
        )
        .await;
    assert!(out.starts_with("Error:"), "{out}");
    assert!(
        out.contains("content"),
        "the missing field has to appear in the error: {out}"
    );
}

#[tokio::test]
async fn a_tool_with_a_broken_schema_still_runs() {
    let mut registry = ToolRegistry::new();
    registry.register(Tool {
        name: "broken_schema".to_string(),
        description: "a tool whose schema is junk".to_string(),
        input_schema: json!("this is not a schema"),
        handler: Arc::new(|_ctx: ToolCtx, _args: serde_json::Value| {
            Box::pin(async { Ok("it really ran".to_string()) })
        }),
    });

    let out = registry
        .execute(ctx().await, "broken_schema", json!({"whatever": true}))
        .await;
    assert_eq!(
        out, "it really ran",
        "a broken schema must not block execution: {out}"
    );
}

#[tokio::test]
async fn valid_arguments_pass_the_schema() {
    let registry = handlers::build_default();
    let ctx = ctx().await;
    let out = registry
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
        )
        .await;
    assert!(out.contains("Remembered"), "{out}");
}

#[tokio::test]
async fn a_duplicate_tool_name_does_not_replace_the_first() {
    use std::sync::Arc;

    use serde_json::Value;

    fn fixed(name: &str, answer: &str) -> super::Tool {
        let answer = answer.to_string();
        super::Tool {
            name: name.to_string(),
            description: "for the test".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            handler: Arc::new(move |_ctx: super::ToolCtx, _args: Value| {
                let answer = answer.clone();
                Box::pin(async move { Ok(answer) })
                    as std::pin::Pin<
                        Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>,
                    >
            }),
        }
    }

    let mut registry = super::ToolRegistry::new();
    registry.register(fixed("dup", "the first one"));
    registry.register(fixed("dup", "the second one"));

    assert_eq!(
        registry
            .names()
            .iter()
            .filter(|name| **name == "dup")
            .count(),
        1,
        "there should be exactly one"
    );
    let out = registry
        .execute(ctx().await, "dup", serde_json::json!({}))
        .await;
    assert_eq!(
        out, "the first one",
        "the first one is kept and the second refused"
    );
}

#[tokio::test]
async fn reading_a_memory_page_carries_its_sources() {
    let ctx = ctx().await;
    let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
    let (fact, _) = ctx
        .facts
        .add(
            "Alex",
            "Alice likes the seven o'clock meeting.",
            "user",
            "user",
        )
        .await
        .unwrap();
    jeeves_memory::mirror::mirror_facts(&ctx.facts, &bundle)
        .await
        .unwrap();
    let mut page = bundle.read("alex").unwrap().unwrap();
    page.sources.push("ledger:s1/t3".into());
    bundle.write(&page).expect("write the page");

    let registry = handlers::build_default();
    let out = registry
        .execute(
            ctx.clone(),
            "read_page",
            serde_json::json!({"kind": "memory", "name": "alex"}),
        )
        .await;
    assert!(out.contains("Alex"), "{out}");
    assert!(out.contains("the seven o'clock meeting"), "{out}");
    assert!(
        out.contains("fact:1"),
        "the source has to come along: {out}"
    );

    ctx.facts.delete(fact.id).await.unwrap();
    let stale = registry
        .execute(
            ctx.clone(),
            "read_page",
            serde_json::json!({"kind": "memory", "name": "alex"}),
        )
        .await;
    assert!(stale.contains("excluded from recall"), "{stale}");
    assert!(!stale.contains("seven o'clock"), "{stale}");
    let missing = registry
        .execute(
            ctx,
            "read_page",
            serde_json::json!({"kind": "memory", "name": "nobody"}),
        )
        .await;
    assert!(missing.contains("has no page \"nobody\""), "{missing}");
}

#[test]
fn deferred_hides_the_shelf_until_find_tools_opens_it() {
    let mut view = handlers::build_default().session_view();
    view.defer_all_but_always();
    let names = |registry: &ToolRegistry| -> Vec<String> {
        registry
            .schemas()
            .into_iter()
            .map(|schema| schema.name)
            .collect()
    };

    assert!(
        !names(&view).contains(&"send_message".to_string()),
        "a tool not activated under deferred must not enter the request"
    );
    for always in ["find_tools", "memory", "read_page", "read_file"] {
        assert!(
            names(&view).contains(&always.to_string()),
            "{always} is an always-on tool and must not be hidden"
        );
    }

    let opened = view.activator()("draft");
    assert_eq!(
        opened,
        vec!["send_message".to_string()],
        "the query has to land on the target's description only"
    );
    assert!(
        names(&view).contains(&"send_message".to_string()),
        "once opened, it should appear in the tool table next round"
    );
}

#[tokio::test]
async fn schedule_job_writes_the_declaration_into_home() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let out = registry
        .execute(
            ctx.clone(),
            "schedule_job",
            json!({"name": "morning-news", "cron": "0 8 * * 1-5", "prompt": "give me a tech briefing"}),
        )
        .await;
    assert!(out.contains("saved"), "{out}");
    assert!(
        out.contains("Next run"),
        "a human has to be told when it runs: {out}"
    );
    assert!(
        out.contains("Runtime"),
        "who triggers it has to be clear, or it looks as if it is already scheduled: {out}"
    );
    assert!(
        !ctx.home.join("schedules.json").exists(),
        "the declaration lives in the state database, not in a file"
    );

    let listed = registry
        .execute(ctx.clone(), "list_schedules", json!({}))
        .await;
    assert!(listed.contains("morning-news"), "{listed}");

    let gone = registry
        .execute(
            ctx.clone(),
            "cancel_schedule",
            json!({"name": "morning-news"}),
        )
        .await;
    assert!(gone.contains("deleted"), "{gone}");
    let after = registry
        .execute(ctx.clone(), "list_schedules", json!({}))
        .await;
    assert!(after.contains("No scheduled jobs yet"), "{after}");
}

#[tokio::test]
async fn a_bad_cron_comes_back_as_a_sentence() {
    let registry = handlers::build_default();
    let ctx = ctx().await;

    let error = registry
        .execute(
            ctx.clone(),
            "schedule_job",
            json!({"name": "x", "cron": "0 8 * *", "prompt": "y"}),
        )
        .await;
    assert!(error.starts_with("Error"), "{error}");
    assert!(error.contains("five fields"), "{error}");
    let listed = registry
        .execute(ctx.clone(), "list_schedules", json!({}))
        .await;
    assert!(
        listed.contains("No scheduled jobs yet"),
        "nothing may be left behind when it is refused: {listed}"
    );
}

#[test]
fn every_builtin_tool_declares_a_schema_it_can_honour() {
    const JSON_TYPES: &[&str] = &[
        "object", "array", "string", "number", "integer", "boolean", "null",
    ];
    let registry = handlers::build_default();
    let names = registry.names();
    assert!(!names.is_empty(), "the tool list must not be empty");

    let mut seen = std::collections::BTreeSet::new();
    for name in &names {
        assert!(
            seen.insert(name.to_string()),
            "the tool name is duplicated: {name} — register would overwrite silently"
        );
        let tool = registry
            .get(name)
            .expect("every tool in names() has to be retrievable");
        assert!(
            tool.description.trim().len() > 4,
            "{name}'s description is empty or too short — that sentence is how the model decides whether to use it"
        );
        let schema = &tool.input_schema;
        assert_eq!(
            schema["type"], "object",
            "{name}'s schema has to be type=object at the top"
        );
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{name} has no properties"));
        if let Some(required) = schema["required"].as_array() {
            for field in required {
                let field = field.as_str().expect("required must hold strings");
                assert!(
                    properties.contains_key(field),
                    "{name} lists {field} in required but not in properties — the model can never fill it in right"
                );
            }
        }
        let declared = schema["type"].as_str().expect("type is a string");
        assert!(
            JSON_TYPES.contains(&declared),
            "{name}'s type is {declared}"
        );

        for (field, spec) in properties {
            let Some(kind) = spec["type"].as_str() else {
                assert!(
                    !spec.is_null(),
                    "{name}.{field} carries no type information, so the model cannot fill it in"
                );
                continue;
            };
            assert!(
                JSON_TYPES.contains(&kind),
                "{name}.{field} is typed {kind}, which is not a JSON type name"
            );
        }
    }
}

#[tokio::test]
async fn readonly_drops_the_exclusive_tools_and_nothing_else() {
    let mut registry = handlers::build_default();
    let mut exclusive = crate::handlers::find_tools();
    exclusive.name = "pretend_write".to_string();
    registry.register_exclusive(exclusive);

    let read_only = registry.readonly();
    assert!(
        read_only.get("pretend_write").is_none(),
        "an exclusive tool has to be taken away"
    );
    assert!(
        !read_only.is_exclusive("pretend_write"),
        "once taken away it must not count as a barrier"
    );
    assert!(
        read_only.get("read_file").is_some() && read_only.get("glob").is_some(),
        "not one read-only tool may go missing"
    );
    assert!(
        registry.get("pretend_write").is_some(),
        "the original table must not change: readonly() copies"
    );
    assert!(
        read_only.names().len() + 1 == registry.names().len(),
        "the one missing is exactly the exclusive tool"
    );
}

fn typed_tool() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Tool {
        name: "typed".to_string(),
        description: "echoes its arguments".to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "count": {"type": "integer"},
                "flag": {"type": "boolean"},
                "items": {"type": "array", "items": {"type": "string"}},
                "label": {"type": "string"}
            },
            "required": ["count"]
        }),
        handler: Arc::new(|_ctx: ToolCtx, args: serde_json::Value| {
            Box::pin(async move { Ok(format!("ran with {args}")) })
        }),
    });
    registry
}

#[tokio::test]
async fn scalars_sent_as_strings_are_parsed_to_the_schema_types() {
    let out = typed_tool()
        .execute(
            ctx().await,
            "typed",
            json!({"count": "5", "flag": "true", "items": "[\"a\",\"b\"]", "label": "7"}),
        )
        .await;
    assert_eq!(
        out, r#"ran with {"count":5,"flag":true,"items":["a","b"],"label":"7"}"#,
        "a string property keeps its string: {out}"
    );
}

#[tokio::test]
async fn a_string_that_is_not_the_declared_type_is_still_rejected() {
    for count in [json!("abc"), json!("1.5"), json!("[1]")] {
        let out = typed_tool()
            .execute(ctx().await, "typed", json!({ "count": count }))
            .await;
        assert!(
            out.starts_with("Error: the arguments do not match the typed schema"),
            "{count}: {out}"
        );
    }
}

#[tokio::test]
async fn a_number_for_a_string_property_is_not_coerced() {
    let out = typed_tool()
        .execute(ctx().await, "typed", json!({"count": 1, "label": 7}))
        .await;
    assert!(out.contains("schema") && out.contains("label"), "{out}");
}

fn named_tools(names: &[&str]) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for name in names {
        let label = name.to_string();
        registry.register(Tool {
            name: name.to_string(),
            description: "reports its own name".to_string(),
            input_schema: json!({"type": "object"}),
            handler: Arc::new(move |_ctx: ToolCtx, _args: serde_json::Value| {
                let label = label.clone();
                Box::pin(async move { Ok(format!("ran {label}")) })
            }),
        });
    }
    registry
}

#[tokio::test]
async fn a_tool_name_in_another_spelling_runs_the_registered_tool() {
    let registry = named_tools(&["read_file", "grep"]);
    for spelled in [
        "ReadFile",
        "readFile",
        "READ_FILE",
        "read-file",
        "read file",
        "read_file_tool",
        "read_file\" parameter=\"path",
    ] {
        let out = registry.execute(ctx().await, spelled, json!({})).await;
        assert_eq!(out, "ran read_file", "{spelled}: {out}");
    }
}

#[tokio::test]
async fn a_tool_name_that_is_not_a_spelling_variant_stays_unknown() {
    let registry = named_tools(&["read_file"]);
    for name in ["write_file", "reed_file", "tool", "\"read_file"] {
        let out = registry.execute(ctx().await, name, json!({})).await;
        assert!(out.starts_with("Error: no tool named"), "{name}: {out}");
    }
}

#[tokio::test]
async fn a_tool_name_matching_two_tools_is_not_repaired() {
    let registry = named_tools(&["read_file", "read-file-tool"]);
    let out = registry.execute(ctx().await, "ReadFile", json!({})).await;
    assert!(out.starts_with("Error: no tool named 'ReadFile'"), "{out}");
}
