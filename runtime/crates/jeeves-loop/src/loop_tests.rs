//! Tests for the loop.
//!
//! A mock provider scripts exactly what the model says each turn, verifying the guardrails
//! one by one: natural end, tool round trip, errors not fatal, and a long run of distinct tools.
//! Scripted model: what the model says is entirely decided by the test.

use std::sync::{Arc, Mutex};

use jeeves_provider::mock::Mock;
use jeeves_provider::{ContentBlock, Message, Role, StopReason, Usage};
use jeeves_tools::handlers;
use serde_json::json;

use crate::{LoopEvent, Turn, run};

async fn ctx() -> jeeves_tools::ToolCtx {
    let dir = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .expect("open the store");
    let _ = dir.keep();
    jeeves_tools::ToolCtx {
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
        turn_limits: std::sync::Arc::new(jeeves_tools::web::TurnLimits::default()),
        session_id: "test".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home: std::path::PathBuf::from(".jeeves"),
        on_delta: None,
        web_receipt: None,
    }
}

async fn turn<'a>(
    client: &'a Mock,
    tools: &'a jeeves_tools::ToolRegistry,
    ctx: jeeves_tools::ToolCtx,
    observer: Option<crate::Observer>,
) -> crate::LoopResult {
    run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "remember that alex likes morning meetings".to_string(),
        tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop should finish")
}

#[tokio::test]
async fn words_added_mid_turn_join_the_next_model_call_and_do_not_rerun_tools() {
    let steering = crate::Steering::new();
    let mock = Mock::new(vec![
        Mock::tool_use("call-1", "glob", json!({ "path": "." })),
        Mock::text("read it all."),
    ]);
    let tools = handlers::build_default();

    let observer: Option<crate::Observer> = {
        let steering = steering.clone();
        Some(Arc::new(move |event: LoopEvent| {
            if let LoopEvent::Llm { iteration: 1, .. } = event {
                steering
                    .lock()
                    .expect("lock")
                    .push("have a look at the README too");
            }
        }))
    };

    let result = run(Turn {
        system_prefix: None,
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "have a look at the directory".to_string(),
        user_images: vec![],
        tools: &tools,
        ctx: ctx().await,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: Some(steering.clone()),
        resume: None,
    })
    .await
    .expect("the loop finishes");

    assert_eq!(
        result.tool_calls.len(),
        1,
        "the tool should run exactly once"
    );
    let received = mock.received.lock().expect("lock");
    assert_eq!(received.len(), 2, "two sentences = two model calls");
    let last = received[1]
        .messages
        .last()
        .expect("the second call is not empty");
    std::assert_matches!(last.role, Role::User);
    assert!(
        last.content
            .iter()
            .any(|block| matches!(block, ContentBlock::Text { text } if text == "have a look at the README too")),
        "{:?}",
        last.content
    );
    assert_eq!(
        steering.lock().expect("lock").all(),
        vec!["have a look at the README too".to_string()]
    );
}

#[tokio::test]
async fn no_tool_call_ends_the_turn_immediately() {
    let mock = Mock::new(vec![Mock::text("fine, noted.")]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;

    assert_eq!(result.reply, "fine, noted.");
    assert_eq!(result.iterations, 1);
    assert!(result.tool_calls.is_empty());
    assert_eq!(result.usage.input_tokens, 10);
    let sent = mock.received.lock().unwrap()[0].clone();
    assert!(
        sent.messages
            .iter()
            .any(|m| m.text().contains("alex likes morning meetings"))
    );
    assert_eq!(sent.system.as_deref(), Some("You are Jeeves"));
}

#[tokio::test]
async fn tool_call_round_trip_feeds_result_back() {
    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
        ),
        Mock::text("already noted."),
    ]);
    let tools = handlers::build_default();
    let ctx = ctx().await;
    let result = turn(&mock, &tools, ctx.clone(), None).await;

    assert_eq!(result.iterations, 2);
    assert_eq!(result.reply, "already noted.");
    assert_eq!(result.tool_calls.len(), 1);
    assert!(
        result.tool_calls[0].ok(),
        "memory(save) must not fail: {}",
        result.tool_calls[0].output
    );

    let second = mock.received.lock().unwrap()[1].clone();
    let has_tool_result = second.messages.iter().any(|m| {
        m.content
            .iter()
            .any(|b| matches!(b, jeeves_provider::ContentBlock::ToolResult { .. }))
    });
    assert!(
        has_tool_result,
        "the second round's messages should carry a tool_result"
    );
    assert_eq!(
        ctx.facts.search("morning meeting", 4).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn tool_error_becomes_text_and_the_loop_continues() {
    let mock = Mock::new(vec![
        Mock::tool_use("tu_1", "memory", json!({"action": "save", "subject": 42})),
        Mock::text("the arguments look wrong, could you say that again?"),
    ]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;

    assert!(!result.tool_calls[0].ok());
    assert_eq!(
        result.iterations, 2,
        "after an error the loop should carry on, not fall over"
    );
    assert_eq!(
        result.reply,
        "the arguments look wrong, could you say that again?"
    );
    let second = mock.received.lock().unwrap()[1].clone();
    let text = second
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            jeeves_provider::ContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(text.starts_with("Error:"), "{text}");
}

#[tokio::test]
async fn unknown_tool_name_is_surfaced_not_fatal() {
    let mock = Mock::new(vec![
        Mock::tool_use("tu_1", "hack_the_planet", json!({})),
        Mock::text("I do not have that tool."),
    ]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert!(
        result.tool_calls[0]
            .output
            .contains("no tool named 'hack_the_planet'")
    );
    assert_eq!(result.iterations, 2);
}

#[tokio::test]
async fn a_long_sequence_of_distinct_tools_runs_until_the_model_finishes() {
    let mut responses: Vec<_> = (1..=66)
        .map(|step| Mock::tool_use(&format!("tool-{step}"), "echo", json!({"step": step})))
        .collect();
    responses.push(Mock::text("All steps completed."));
    let mock = Mock::new(responses);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("echo", 0, "evidence"),
        jeeves_tools::ToolEffect::Read,
    );
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.iterations, 67);
    assert_eq!(result.tool_calls.len(), 66);
    assert_eq!(result.reply, "All steps completed.");
    assert!(
        !mock
            .received
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .tools
            .is_empty()
    );
}

#[tokio::test]
async fn an_active_context_overflow_retries_only_the_model_request() {
    let mut outcomes: Vec<_> = (1..=6)
        .map(|step| {
            Ok(Mock::tool_use(
                &format!("fetch-{step}"),
                "fetch",
                json!({"step": step}),
            ))
        })
        .collect();
    outcomes.push(Mock::context_overflow());
    outcomes.push(Ok(Mock::text("The work is complete.")));
    let mock = Mock::with_outcomes(outcomes);
    let evidence = "Important evidence in the original tool response. ".repeat(300);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("fetch", 0, &evidence),
        jeeves_tools::ToolEffect::Read,
    );
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.reply, "The work is complete.");
    assert_eq!(result.tool_calls.len(), 6);
    assert!(result.tool_calls.iter().all(|call| call.output == evidence));
    let requests = mock.received.lock().unwrap();
    assert_eq!(requests.len(), 8);
    let retry = &requests[7].messages;
    assert!(
        jeeves_provider::tokens::estimate_messages(retry)
            < jeeves_provider::tokens::estimate_messages(&requests[6].messages)
    );
    let replay = serde_json::to_string(retry).unwrap();
    assert!(replay.contains("remember that alex likes morning meetings"));
    assert!(replay.contains("Context compacted"));
    assert!(replay.contains("fetch-6"));
    assert!(
        result
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|block| {
                matches!(block, ContentBlock::ToolResult { tool_use_id, content }
            if tool_use_id == "fetch-1" && content == &evidence)
            })
    );
}

#[tokio::test]
async fn repeated_batches_eventually_yield_for_review() {
    let mock = Mock::new(
        (1..=5)
            .map(|step| Mock::tool_use(&format!("repeat-{step}"), "fetch", json!({})))
            .collect(),
    );
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("fetch", 0, "same evidence"),
        jeeves_tools::ToolEffect::Read,
    );
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.tool_calls.len(), 5);
    assert!(result.stop_detail.unwrap().contains("repeated warnings"));
}

#[tokio::test]
async fn a_truncated_response_cannot_execute_tools() {
    let mut truncated = Mock::tool_use("call-truncated", "write", json!({}));
    truncated.stop_reason = StopReason::MaxTokens;
    let mock = Mock::new(vec![truncated.clone(), truncated.clone(), truncated]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("write", 0, "would be a side effect"),
        jeeves_tools::ToolEffect::Read,
    );
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.stop_reason, StopReason::MaxTokens);
    assert!(result.tool_calls.is_empty());
    assert!(
        result
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .all(|b| !matches!(b, ContentBlock::ToolUse { .. }))
    );
}

#[tokio::test]
async fn the_token_budget_also_turns_the_next_round_into_a_wrap_up() {
    let mock = Mock::new(vec![
        Mock::tool_use("tu_1", "memory", json!({"action": "list"})),
        Mock::text(
            "the budget is up, so I am wrapping up: the time was read, the rest of the cleanup is not done.",
        ),
    ]);
    let tools = handlers::build_default();
    let ctx = ctx().await;

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "what time is it".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: Some(20),
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the budget closing should not error");

    assert_eq!(
        result.iterations, 2,
        "the first round as usual, the second goes straight to closing"
    );
    let requests = mock.received.lock().expect("lock");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1].tools.is_empty(),
        "the budget wrapping-up round must get no more tools: {} of them",
        requests[1].tools.len()
    );
    let tail = serde_json::to_string(&requests[1].messages).expect("serialise");
    assert!(
        tail.contains("token budget is spent"),
        "it has to say plainly that the budget stopped it: {tail:.200}"
    );
}

#[tokio::test]
async fn without_a_token_budget_the_loop_keeps_its_tools() {
    let mock = Mock::new(vec![
        Mock::tool_use("tu_1", "memory", json!({"action": "list"})),
        Mock::text("done"),
    ]);
    let tools = handlers::build_default();
    let ctx = ctx().await;

    let _ = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "what time is it".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("two rounds and it should be over");

    let requests = mock.received.lock().expect("lock");
    assert!(
        !requests[1].tools.is_empty(),
        "with no budget the second round carries tools as usual"
    );
    let tail = serde_json::to_string(&requests[1].messages).expect("serialise");
    assert!(
        !tail.contains("Budget note"),
        "no budget means no closing instruction"
    );
}

#[tokio::test]
async fn observer_sees_llm_and_tool_events_in_order() {
    #[derive(Default)]
    struct Log(Vec<String>);
    let log = Arc::new(Mutex::new(Log::default()));
    let sink = log.clone();

    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        ),
        Mock::text("noted"),
    ]);
    let tools = handlers::build_default();
    let observer = move |event: LoopEvent| {
        let name = match event {
            LoopEvent::Llm {
                iteration,
                stop_reason,
                ..
            } => {
                format!(
                    "llm#{iteration}={}",
                    match stop_reason {
                        StopReason::ToolUse => "tool_use",
                        StopReason::EndTurn => "end_turn",
                        _ => "other",
                    }
                )
            }
            LoopEvent::Text { .. } => "text".to_string(),
            LoopEvent::ToolStart { name, .. } => format!("tool_start:{name}"),
            LoopEvent::ToolDelta { chunk, .. } => format!("tool_delta:{chunk}"),
            LoopEvent::Tool { name, .. } => format!("tool:{name}"),
        };
        sink.lock().unwrap().0.push(name);
    };

    let result = turn(&mock, &tools, ctx().await, Some(Arc::new(observer))).await;
    assert_eq!(result.iterations, 2);

    let events = &log.lock().unwrap().0;
    assert_eq!(
        events,
        &vec![
            "llm#1=tool_use".to_string(),
            "tool_start:memory".to_string(),
            "tool:memory".to_string(),
            "llm#2=end_turn".to_string()
        ]
    );
}

#[tokio::test]
async fn streaming_emits_text_deltas_in_order() {
    let mock = Mock::streaming(vec![Mock::text("all right, noted.")]);
    let tools = handlers::build_default();

    let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = deltas.clone();
    let on_text: jeeves_provider::TextSink = Arc::new(move |delta: &str| {
        sink.lock().unwrap().push(delta.to_string());
    });

    let events: Arc<Mutex<Vec<LoopEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let observer_sink = events.clone();
    let observer: crate::Observer = Arc::new(move |event: LoopEvent| {
        observer_sink.lock().unwrap().push(event);
    });

    let ctx = ctx().await;
    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "hi".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: Some(observer),
        on_text: Some(on_text),
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop finishes");

    assert!(
        mock.streamed.lock().unwrap().iter().all(|x| *x),
        "it really did go through streaming"
    );
    let collected = deltas.lock().unwrap().join("");
    assert_eq!(
        collected, "all right, noted.",
        "the joined deltas should be the whole text"
    );
    assert!(
        deltas.lock().unwrap().len() > 1,
        "there should be several deltas, not one whole paragraph"
    );

    let text_events = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            LoopEvent::Text { delta } => Some(delta.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(text_events, "all right, noted.");
    assert_eq!(
        result.reply, "all right, noted.",
        "the final reply should still be whole"
    );
}

#[tokio::test]
async fn streaming_works_across_tool_rounds() {
    let mock = Mock::streaming(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        ),
        Mock::text("noted."),
    ]);
    let tools = handlers::build_default();

    let deltas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = deltas.clone();
    let on_text: jeeves_provider::TextSink = Arc::new(move |d: &str| {
        sink.lock().unwrap().push(d.to_string());
    });

    let ctx = ctx().await;
    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "hi".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: Some(on_text),
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop finishes");

    assert_eq!(result.iterations, 2);
    assert_eq!(
        deltas.lock().unwrap().join(""),
        "noted.",
        "the tool round has no text, so no delta should be produced"
    );
    assert!(result.tool_calls[0].ok());
}

#[tokio::test]
async fn a_cancelled_interrupt_stops_the_loop_before_any_model_call() {
    let mock = Mock::new(vec![Mock::text(
        "this sentence never gets its turn to be said.",
    )]);
    let tools = handlers::build_default();

    let interrupt = crate::Interrupt::new();
    interrupt.cancel();

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "hi".to_string(),
        tools: &tools,
        ctx: ctx().await,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: Some(interrupt),
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("an interrupt is not an error");

    assert!(result.interrupted);
    assert_eq!(result.iterations, 0, "not one model call should happen");
    assert_eq!(result.reply, "(this turn was interrupted.)");
    assert!(
        mock.received.lock().unwrap().is_empty(),
        "calling the model after the cancel token was flipped means it did not listen"
    );
}

#[tokio::test]
async fn an_interrupt_between_tool_rounds_keeps_what_already_happened() {
    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "a", "content": "b"}),
        ),
        Mock::text("this sentence should not appear."),
    ]);
    let tools = handlers::build_default();
    let ctx = ctx().await;

    let interrupt = crate::Interrupt::new();
    let first_tool_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = first_tool_fired.clone();
    let interrupt_for_observer = interrupt.clone();
    let observer: crate::Observer = Arc::new(move |event| {
        if matches!(event, crate::LoopEvent::Tool { .. }) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            interrupt_for_observer.cancel();
        }
    });

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "hi".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: Some(observer),
        on_text: None,
        interrupt: Some(interrupt),
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("an interrupt is not an error");

    assert!(first_tool_fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(result.interrupted);
    assert_eq!(
        result.tool_calls.len(),
        1,
        "the first round's tool really did run"
    );
    assert_eq!(result.iterations, 1, "the second model call never happened");
    assert_eq!(result.reply, "(this turn was interrupted.)");
}

#[tokio::test]
async fn the_stall_guard_warns_the_model_but_keeps_real_tool_output() {
    let args = json!({"action": "search", "query": "something that cannot be found"});
    let mock = Mock::new(vec![
        Mock::tool_use("tu_1", "memory", args.clone()),
        Mock::tool_use("tu_2", "memory", args.clone()),
        Mock::tool_use("tu_3", "memory", args.clone()),
        Mock::text("done."),
    ]);
    let tools = handlers::build_default();

    let result = turn(&mock, &tools, ctx().await, None).await;

    assert!(
        result.guard.hits >= 1,
        "the third identical call must trip the guard"
    );
    assert!(
        result.guard.note.is_some(),
        "the note about the trip has to be kept"
    );

    let warned = result.messages.iter().any(|message| {
        message.content.iter().any(|block| {
            matches!(block, jeeves_provider::ContentBlock::ToolResult { content, .. }
                     if content.contains("[guard]"))
        })
    });
    assert!(warned, "the warning has to land where the model can see it");

    assert!(
        result
            .tool_calls
            .iter()
            .all(|call| !call.output.contains("[guard]")),
        "tool_calls should hold the real result"
    );
    assert!(
        result.tool_calls.iter().all(|call| call.ok()),
        "all three calls were themselves successful"
    );
}

#[tokio::test]
async fn an_oversized_tool_result_becomes_a_stub_before_the_next_request() {
    use jeeves_tools::{BoxFut, Tool, ToolRegistry};
    use serde_json::Value;

    let home = tempfile::tempdir().expect("temp dir");
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .expect("open the store");
    let ctx = jeeves_tools::ToolCtx {
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
        turn_limits: std::sync::Arc::new(jeeves_tools::web::TurnLimits::default()),
        session_id: "budget".to_string(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool.clone()),
        home: home.path().to_path_buf(),
        on_delta: None,
        web_receipt: None,
    };

    let big: Tool = Tool {
        name: "mcp__demo__fetch".to_string(),
        description: "answer with one big block of text".to_string(),
        input_schema: json!({"type": "object", "properties": {}}),
        handler: Arc::new(|_ctx: jeeves_tools::ToolCtx, _args: Value| {
            Box::pin(async move { Ok("z".repeat(100_000)) }) as BoxFut
        }),
    };
    let mut tools = ToolRegistry::new();
    tools.register(big);

    let mock = Mock::new(vec![
        Mock::tool_use("call-1", "mcp__demo__fetch", json!({})),
        Mock::text("read it all."),
    ]);

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "let us have a look at that text".to_string(),
        tools: &tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: crate::budget::ToolResultBudget {
            total_chars: 10_000,
            per_result_chars: 5_000,
            keep_recent: 0,
        },
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop finishes");

    assert_eq!(result.reply, "read it all.");
    let requests = mock.received.lock().unwrap();
    assert_eq!(requests.len(), 2, "one tool round trip = two model calls");

    let second = &requests[1];
    let stub = second
        .messages
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            jeeves_provider::ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("the second request should carry the tool result");
    assert!(
        stub.contains("truncated") && stub.chars().count() < 5_000,
        "an over-limit result should be swapped for a stub: {} characters",
        stub.chars().count()
    );
    assert!(stub.contains("full output at spill/"), "{stub}");

    let relative = stub
        .split("full output at ")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .expect("the path");
    let saved = std::fs::read_to_string(home.path().join(relative)).expect("read the spill back");
    assert_eq!(saved.chars().count(), 100_000);

    assert!(relative.starts_with("spill/"), "{relative}");
}
#[tokio::test]
async fn a_checkpoint_lands_at_every_tool_results_boundary() {
    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
        ),
        Mock::text("noted."),
    ]);
    let tools = handlers::build_default();
    let checkpoints: Arc<Mutex<Vec<crate::Checkpoint>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = {
        let checkpoints = checkpoints.clone();
        Arc::new(move |cp: crate::Checkpoint| {
            let checkpoints = checkpoints.clone();
            Box::pin(async move {
                checkpoints.lock().unwrap().push(cp);
                Ok(())
            })
                as std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        }) as crate::CheckpointSink
    };

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "remember that alex likes morning meetings".to_string(),
        tools: &tools,
        ctx: ctx().await,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: Some(sink),
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop finishes");

    assert_eq!(result.iterations, 2);
    let cps = checkpoints.lock().unwrap();
    assert!(cps.iter().all(|cp| cp.round_id == result.round_id));
    assert!(!result.round_id.is_empty());
    assert_eq!(cps.len(), 3, "initial input, execution intent, tool result");
    assert_eq!(cps[0].iterations, 0);
    assert!(cps[0].operations.is_empty());
    assert_eq!(cps[1].operations.len(), 1);
    assert_eq!(cps[1].operations[0].tool_use_id, "tu_1");
    assert!(cps[1].operations[0].output.is_none());
    assert!(cps[2].operations[0].output.is_some());
    let last = cps[2].messages.last().expect("there is a result block");
    std::assert_matches!(
        last.content.first(),
        Some(jeeves_provider::ContentBlock::ToolResult { .. })
    );
}

#[tokio::test]
async fn a_resumed_turn_continues_without_rerunning_finished_tools() {
    let mock = Mock::new(vec![Mock::text(
        "carrying on from the last round, it is done.",
    )]);
    let tools = handlers::build_default();

    let messages = vec![
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "remember that alex likes morning meetings".to_string(),
            }],
        },
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: "tu_1".to_string(),
                name: "memory".to_string(),
                input: json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
                extra: None,
            }],
        },
        Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "tu_1".to_string(),
                content: "saved".to_string(),
            }],
        },
    ];

    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: &mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: String::new(),
        tools: &tools,
        ctx: ctx().await,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: Some(crate::Resume {
            round_id: "test-round".into(),
            messages,
            completed_iterations: 3,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
                ..Usage::default()
            },
        }),
    })
    .await
    .expect("the resume should finish");

    assert_eq!(result.round_id, "test-round");
    assert_eq!(result.reply, "carrying on from the last round, it is done.");
    assert_eq!(result.iterations, 4, "3 finished iterations plus 1 new one");
    assert!(
        result.tool_calls.is_empty(),
        "a tool that already ran never runs again"
    );
    assert_eq!(
        result.usage.input_tokens, 110,
        "the two ledgers added together (100 + 10)"
    );
    let sent = mock.received.lock().unwrap()[0].clone();
    assert_eq!(sent.messages.len(), 3);
}

fn slow_tool(name: &str, ms: u64, answer: &str) -> jeeves_tools::Tool {
    let answer = answer.to_string();
    jeeves_tools::Tool {
        name: name.to_string(),
        description: "for the test: sleep a while, then answer".to_string(),
        input_schema: json!({"type": "object", "properties": {}, "required": []}),
        handler: Arc::new(move |_ctx: jeeves_tools::ToolCtx, _args| {
            let answer = answer.clone();
            Box::pin(async move {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                Ok(answer)
            })
        }),
    }
}

fn two_calls(first: (&str, &str), second: (&str, &str)) -> jeeves_provider::CreateResponse {
    use jeeves_provider::{ContentBlock, CreateResponse, StopReason, Usage};
    CreateResponse {
        stop_reason: StopReason::ToolUse,
        usage: Usage::default(),
        content: vec![
            ContentBlock::ToolUse {
                id: first.0.to_string(),
                name: first.1.to_string(),
                input: json!({}),
                extra: None,
            },
            ContentBlock::ToolUse {
                id: second.0.to_string(),
                name: second.1.to_string(),
                input: json!({}),
                extra: None,
            },
        ],
    }
}

async fn run_one_round(
    mock: &Mock,
    tools: &jeeves_tools::ToolRegistry,
) -> (crate::LoopResult, std::time::Duration) {
    run_with(mock, tools, None).await
}

async fn run_with(
    mock: &Mock,
    tools: &jeeves_tools::ToolRegistry,
    observer: Option<crate::Observer>,
) -> (crate::LoopResult, std::time::Duration) {
    let ctx = ctx().await;
    jeeves_provider::tokens::estimate_text("warm");
    let started = std::time::Instant::now();
    let result = run(Turn {
        system_prefix: None,
        user_images: Vec::new(),
        client: mock,
        model: "test-model",
        system: "You are Jeeves".to_string(),
        history: vec![],
        user_message: "do two things at once".to_string(),
        tools,
        ctx,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 1024,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer,
        on_text: None,
        interrupt: None,
        checkpoint: None,
        steering: None,
        resume: None,
    })
    .await
    .expect("the loop finishes");
    (result, started.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_tools_in_one_round_run_together() {
    let mock = Mock::new(vec![
        two_calls(("call-1", "slow_a"), ("call-2", "slow_b")),
        Mock::text("everything is done."),
    ]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("slow_a", 200, "A"),
        jeeves_tools::ToolEffect::Read,
    );
    tools.register_with_effect(
        slow_tool("slow_b", 200, "B"),
        jeeves_tools::ToolEffect::Read,
    );

    let (result, elapsed) = run_one_round(&mock, &tools).await;

    assert_eq!(result.tool_calls.len(), 2);
    assert!(
        elapsed < std::time::Duration::from_millis(360),
        "two 200 ms read-only tools have to run concurrently (serial would be 400 ms+): {elapsed:?}"
    );
}

#[tokio::test]
async fn results_are_reported_in_request_order_not_completion_order() {
    let mock = Mock::new(vec![
        two_calls(("call-slow", "slow_a"), ("call-fast", "slow_b")),
        Mock::text("everything is done."),
    ]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("slow_a", 120, "A"),
        jeeves_tools::ToolEffect::Read,
    );
    tools.register_with_effect(slow_tool("slow_b", 5, "B"), jeeves_tools::ToolEffect::Read);

    let completion_order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = completion_order.clone();
    let observer: crate::Observer = Arc::new(move |event| {
        if let crate::LoopEvent::Tool {
            tool_use_id,
            success,
            ..
        } = event
        {
            assert!(success);
            observed.lock().unwrap().push(tool_use_id);
        }
    });
    let (result, _) = run_with(&mock, &tools, Some(observer)).await;
    assert_eq!(
        *completion_order.lock().unwrap(),
        ["call-fast", "call-slow"]
    );

    let names: Vec<&str> = result.tool_calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["slow_a", "slow_b"],
        "the tool ledger keeps request order"
    );
    let sent = mock.received.lock().unwrap().pop().unwrap();
    let blocks = &sent
        .messages
        .last()
        .expect("the last message is the tool result")
        .content;
    let ids: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ids,
        vec!["call-slow", "call-fast"],
        "what goes back to the model keeps order too"
    );
}

#[tokio::test]
async fn an_exclusive_tool_is_a_barrier() {
    let mock = Mock::new(vec![
        two_calls(("call-1", "reader"), ("call-2", "writer")),
        two_calls(("call-3", "reader2"), ("call-4", "reader3")),
        Mock::text("everything is done."),
    ]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("reader", 120, "R"),
        jeeves_tools::ToolEffect::Read,
    );
    tools.register_exclusive(slow_tool("writer", 120, "W"));
    tools.register_with_effect(
        slow_tool("reader2", 120, "R2"),
        jeeves_tools::ToolEffect::Read,
    );
    tools.register_with_effect(
        slow_tool("reader3", 120, "R3"),
        jeeves_tools::ToolEffect::Read,
    );

    let (result, elapsed) = run_one_round(&mock, &tools).await;

    assert_eq!(result.tool_calls.len(), 4);
    assert!(
        elapsed >= std::time::Duration::from_millis(330),
        "an exclusive tool has to be a barrier between what comes before and after: {elapsed:?}"
    );
}

#[tokio::test]
async fn a_streaming_tool_emits_deltas_and_still_returns_the_real_result() {
    use std::sync::Mutex;

    let events: Arc<Mutex<Vec<crate::LoopEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let collected = events.clone();
    let observer: crate::Observer = Arc::new(move |event| {
        collected.lock().unwrap().push(event);
    });

    let mock = Mock::new(vec![
        Mock::tool_use("call-1", "streamer", json!({})),
        Mock::text("received."),
    ]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register(jeeves_tools::Tool {
        name: "streamer".to_string(),
        description: "report as it goes".to_string(),
        input_schema: json!({"type": "object", "properties": {}, "required": []}),
        handler: Arc::new(|ctx: jeeves_tools::ToolCtx, _args| {
            Box::pin(async move {
                if let Some(sink) = &ctx.on_delta {
                    sink("the first paragraph");
                    sink("the second paragraph");
                }
                Ok("the full result".to_string())
            })
        }),
    });

    let (result, _) = run_with(&mock, &tools, Some(observer)).await;

    let events = events.lock().unwrap();
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            crate::LoopEvent::ToolDelta { chunk, .. } => Some(chunk.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        deltas,
        vec!["the first paragraph", "the second paragraph"],
        "deltas go out in arrival order"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            crate::LoopEvent::ToolDelta { tool_use_id, .. } if tool_use_id == "call-1"
        )),
        "a delta has to carry the call id, or the front end cannot tell which row to update when parallel"
    );
    let finished: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            crate::LoopEvent::Tool { output, .. } => Some(output.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        finished,
        vec!["the full result"],
        "the Tool event still carries only the final result"
    );
    assert_eq!(result.tool_calls[0].output, "the full result");
}

#[tokio::test]
async fn uncertain_effect_stops_remaining_tools_and_keeps_the_intent_checkpoint() {
    let mut proposal = two_calls(("saved", "save_report"), ("unknown", "uncertain"));
    proposal
        .content
        .extend(Mock::tool_use("later", "must_not_run", json!({})).content);
    let mock = Mock::new(vec![proposal, Mock::text("must not be called")]);
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register(slow_tool("save_report", 0, "Error: sample report"));
    tools.register_exclusive(jeeves_tools::Tool {
        name: "uncertain".into(),
        description: "unknown remote effect".into(),
        input_schema: json!({"type":"object"}),
        handler: Arc::new(|ctx, _| {
            Box::pin(async move {
                ctx.on_uncertain.as_ref().unwrap()("remote outcome unknown");
                Ok("Error: remote outcome unknown".into())
            })
        }),
    });
    let capture = ran.clone();
    tools.register_exclusive(jeeves_tools::Tool {
        name: "must_not_run".into(),
        description: "later effect".into(),
        input_schema: json!({"type":"object"}),
        handler: Arc::new(move |_, _| {
            let capture = capture.clone();
            Box::pin(async move {
                capture.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok("ran".into())
            })
        }),
    });
    let checkpoints = Arc::new(Mutex::new(Vec::new()));
    let captured = checkpoints.clone();
    let sink: crate::CheckpointSink = Arc::new(move |checkpoint| {
        let captured = captured.clone();
        Box::pin(async move {
            captured.lock().unwrap().push(checkpoint);
            Ok(())
        })
    });
    let result = run(Turn {
        system_prefix: None,
        user_images: vec![],
        client: &mock,
        model: "test",
        system: "test".into(),
        history: vec![],
        user_message: "work".into(),
        tools: &tools,
        ctx: ctx().await,
        run_limits: None,
        context_budget: None,
        context_window: None,
        max_tokens: 600,
        token_budget: None,
        tool_result_budget: Default::default(),
        observer: None,
        on_text: None,
        interrupt: None,
        checkpoint: Some(sink),
        steering: None,
        resume: None,
    })
    .await
    .unwrap();
    assert_eq!(result.stop_reason, StopReason::UncertainEffect);
    assert_eq!(mock.received.lock().unwrap().len(), 1);
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(result.tool_calls.len(), 2);
    assert!(result.tool_calls[0].ok());
    assert!(!result.tool_calls[1].ok());
    let saved = checkpoints.lock().unwrap();
    assert_eq!(saved.len(), 4);
    assert!(saved[1].operations[0].output.is_none());
    let report = saved[2].operations[0].output.as_ref().unwrap();
    assert!(report.success);
    assert_eq!(report.text, "Error: sample report");
    assert_eq!(saved[3].operations.len(), 1);
    assert_eq!(saved[3].operations[0].tool_use_id, "unknown");
    assert!(saved[3].operations[0].output.is_none());
    let transcript = serde_json::to_string(&saved[3].messages).unwrap();
    assert!(transcript.contains("Error: sample report"));
    assert!(!transcript.contains("must_not_run"));
}

#[tokio::test]
async fn a_call_with_broken_arguments_is_answered_and_the_turn_goes_on() {
    let broken = Mock::tool_use(
        "call-broken",
        "fetch",
        jeeves_provider::tool_input(r#"{"url": "#),
    );
    let mock = Mock::new(vec![broken, Mock::text("fixed it")]);
    let mut tools = jeeves_tools::ToolRegistry::new();
    tools.register_with_effect(
        slow_tool("fetch", 0, "must not run"),
        jeeves_tools::ToolEffect::Read,
    );
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.reply, "fixed it");
    assert_eq!(result.tool_calls.len(), 1);
    let call = &result.tool_calls[0];
    assert!(!call.success);
    assert!(call.output.contains("not valid JSON"), "{}", call.output);
    assert!(!call.output.contains("must not run"), "{}", call.output);
}

#[tokio::test]
async fn a_reply_cut_by_the_output_limit_is_continued() {
    let mut first = Mock::text("The first half, ");
    first.stop_reason = StopReason::MaxTokens;
    let mock = Mock::new(vec![first, Mock::text("and the second half.")]);
    let tools = jeeves_tools::ToolRegistry::new();
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.reply, "The first half, and the second half.");
    let received = mock.received.lock().expect("lock");
    assert_eq!(received.len(), 2);
    let asked = received[1].messages.last().expect("a message").text();
    assert!(
        asked.contains("Continue exactly where it stopped"),
        "{asked}"
    );
}

#[tokio::test]
async fn continuation_after_the_output_limit_is_bounded() {
    let mut cut = Mock::text("more ");
    cut.stop_reason = StopReason::MaxTokens;
    let mock = Mock::new(vec![cut.clone(), cut.clone(), cut]);
    let tools = jeeves_tools::ToolRegistry::new();
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.stop_reason, StopReason::MaxTokens);
    assert_eq!(result.reply, "more more more ");
    assert_eq!(mock.received.lock().expect("lock").len(), 3);
}

/// A reply that only thought: no visible text, no tool call.
fn thinking_only() -> jeeves_provider::CreateResponse {
    jeeves_provider::CreateResponse {
        stop_reason: StopReason::EndTurn,
        usage: Usage::default(),
        content: vec![ContentBlock::Thinking {
            thinking: "the answer is in the tool result".to_string(),
            signature: String::new(),
        }],
    }
}

#[tokio::test]
async fn an_empty_reply_after_a_tool_round_is_asked_once_to_answer() {
    let mock = Mock::new(vec![
        Mock::tool_use(
            "tu_1",
            "memory",
            json!({"action": "save", "subject": "alex", "content": "likes morning meetings"}),
        ),
        Mock::text(""),
        Mock::text("Saved: alex likes morning meetings."),
    ]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;

    assert_eq!(result.reply, "Saved: alex likes morning meetings.");
    let received = mock.received.lock().unwrap();
    assert_eq!(received.len(), 3, "one extra call, no more");
    let nudge = received[2].messages.last().expect("a last message").text();
    assert!(nudge.contains("no visible text"), "{nudge}");
    let blank_text = received[2]
        .messages
        .iter()
        .flat_map(|m| &m.content)
        .any(|block| matches!(block, ContentBlock::Text { text } if text.trim().is_empty()));
    assert!(
        !blank_text,
        "no empty text block is sent back to the provider"
    );
}

#[tokio::test]
async fn a_model_that_stays_empty_ends_the_turn_with_a_notice() {
    let mock = Mock::new(vec![thinking_only(), thinking_only(), Mock::text("unused")]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;

    assert!(
        result.reply.contains("without any text"),
        "the person is told why nothing came back: {}",
        result.reply
    );
    assert_eq!(
        mock.received.lock().unwrap().len(),
        2,
        "only one nudge per turn"
    );
    let roles: Vec<Role> = result.messages.iter().map(|m| m.role).collect();
    assert!(
        roles.windows(2).all(|pair| pair[0] != pair[1]),
        "the transcript alternates roles: {roles:?}"
    );
}

#[tokio::test]
async fn a_reply_with_text_is_never_nudged() {
    let mock = Mock::new(vec![Mock::text("done."), Mock::text("unused")]);
    let tools = handlers::build_default();
    let result = turn(&mock, &tools, ctx().await, None).await;
    assert_eq!(result.reply, "done.");
    assert_eq!(mock.received.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn evidence_gate_uses_model_selected_reads_and_never_streams_the_proposed_reply() {
    use crate::evidence::{self, Claim, Decision, Freshness};
    use jeeves_tools::web::WebReceipt;

    let source = "https://registry.example/packages/widget";
    let mut tools = jeeves_tools::ToolRegistry::new();
    // A host-controlled handler fixture supplies provenance separately from the body.
    tools.register(jeeves_tools::Tool {
        name: "fetch_url".into(),
        description: "Read a package registry source".into(),
        input_schema: json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"]}),
        handler: Arc::new(move |ctx, _| {
            Box::pin(async move {
                let _ = ctx.web_receipt.unwrap().set(WebReceipt::Page {
                    requested: source.into(),
                    effective: Some(source.into()),
                    complete: true,
                });
                Ok("widget stable 2030-11-08 v2.0".into())
            })
        }),
    });
    let mock = Mock::streaming(vec![
        Mock::tool_use("package-read", "fetch_url", json!({"url": source})),
        Mock::text("The latest stable package is v9.9. Install it now."),
    ]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let observer: crate::Observer = Arc::new(move |event| observed.lock().unwrap().push(event));
    let gate = |calls: &[crate::ToolOutcome]| {
        let decision = evidence::resolve(
            calls,
            &"widget stable",
            Freshness::Within(20301108..=20301108),
            |_, effective, body| {
                (effective == source && body == "widget stable 2030-11-08 v2.0").then_some(Claim {
                    entity: "widget stable",
                    recorded_at: Some(20301108),
                    identity: "stable release",
                    value: "v2.0",
                })
            },
            |_| true,
        );
        match decision {
            Decision::Confirmed { value, source, .. } => format!("{value} from {source}"),
            _ => "No verified release".into(),
        }
    };

    let result = crate::run_with_evidence(
        Turn {
            system_prefix: None,
            user_images: Vec::new(),
            client: &mock,
            model: "test-model",
            system: "You are Jeeves".into(),
            history: Vec::new(),
            user_message: "Verify the stable package".into(),
            tools: &tools,
            ctx: ctx().await,
            run_limits: None,
            context_budget: None,
            context_window: None,
            max_tokens: 1024,
            token_budget: None,
            tool_result_budget: Default::default(),
            observer: Some(observer),
            on_text: Some(Arc::new(|_| {
                panic!("unreviewed text must not reach the sink")
            })),
            interrupt: None,
            checkpoint: None,
            steering: None,
            resume: None,
        },
        None,
        Some(&gate),
    )
    .await
    .unwrap();
    assert_eq!(result.reply, format!("v2.0 from {source}"));
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, LoopEvent::Tool { success: true, .. }))
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, LoopEvent::Text { .. }))
    );
}
