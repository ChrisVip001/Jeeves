//! Rebuild durable task context from normalized records, independently of chat summaries.
use crate::Server;
use jeeves_protocol::{ErrorObject, codes};
use serde_json::{Value, json};

fn failed(error: impl std::fmt::Display) -> ErrorObject {
    ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: format!("Cannot assemble durable task handoff; execution stopped: {error}"),
        data: None,
    }
}

pub(crate) async fn load(
    server: &Server,
    turn_id: &str,
    session_id: &str,
) -> Result<String, ErrorObject> {
    let Some(run) = server.runs.for_turn(turn_id).await.map_err(failed)? else {
        if server
            .checkpoints
            .get(turn_id)
            .await
            .map_err(failed)?
            .is_some_and(|checkpoint| checkpoint.run_id.is_some())
        {
            return Err(failed("checkpoint run has no matching execution identity"));
        }
        return Ok(String::new());
    };
    if run
        .session_id
        .as_deref()
        .is_some_and(|owner| owner != session_id)
    {
        return Err(failed("execution session does not match the run"));
    }
    let task = server
        .tasks
        .get(&run.task_id)
        .await
        .map_err(failed)?
        .ok_or_else(|| failed("task is missing"))?;
    crate::task::contract::validate(&task).map_err(failed)?;
    let acceptance: Value = serde_json::from_str(&task.acceptance_json).map_err(failed)?;
    let budget: Value = serde_json::from_str(&task.budget_json).map_err(failed)?;
    let previous_checks: Value = serde_json::from_str(&task.checks_json).map_err(failed)?;
    let plan = jeeves_agent::plan::restore(&server.steps, &task.id)
        .await
        .map_err(failed)?
        .map(|plan| json!({"version":plan.version,"steps":plan.steps}));
    let steps = server.steps.list_by_run(&run.id).await.map_err(failed)?;
    let recent_steps:Vec<Value>=steps.into_iter().rev().take(16).map(|step|json!({
        "sequence":step.seq,"tool":step.tool,"status":step.status,"operationKey":step.idempotency_key
    })).collect();
    let artifacts = server.artifacts.list(&task.id, 16).await.map_err(failed)?;
    let artifacts:Vec<Value>=artifacts.into_iter().map(|artifact|json!({
        "path":artifact.path,"recordedBytes":artifact.bytes,"recordedAt":artifact.at,"runId":artifact.run_id
    })).collect();
    let operations = jeeves_state::operations::ToolOperations::new(server.pool.clone())
        .list(&task.id)
        .await
        .map_err(failed)?;
    let open_todos = operations.iter().rev().find_map(|operation| {
        if operation.tool != "todo_write" || operation.evidence_status() != "succeeded" {
            return None;
        }
        let input: Value = serde_json::from_str(&operation.input_json).ok()?;
        let todos = input.get("todos")?.as_array()?;
        Some(
            todos
                .iter()
                .filter(|todo| {
                    matches!(
                        todo.get("status").and_then(Value::as_str),
                        Some("pending" | "in_progress")
                    )
                })
                .take(20)
                .map(|todo| json!({"content":todo.get("content").and_then(Value::as_str).unwrap_or("").chars().take(400).collect::<String>(),"status":todo.get("status")}))
                .collect::<Vec<_>>(),
        )
    });
    let mut edited = std::collections::BTreeSet::new();
    for operation in &operations {
        if !["edit", "write_file", "apply_patch"].contains(&operation.tool.as_str()) {
            continue;
        }
        let Ok(input) = serde_json::from_str::<Value>(&operation.input_json) else {
            continue;
        };
        if let Some(path) = input.get("path").and_then(Value::as_str) {
            edited.insert(path.chars().take(256).collect::<String>());
        }
        if let Some(patch) = input.get("patch").and_then(Value::as_str) {
            for line in patch.lines() {
                for prefix in [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ] {
                    if let Some(path) = line.strip_prefix(prefix) {
                        edited.insert(path.chars().take(256).collect());
                    }
                }
            }
        }
    }
    let mut sources = Vec::new();
    for op in operations
        .iter()
        .filter(|op| op.evidence_status() == "succeeded")
    {
        let input: Value = serde_json::from_str(&op.input_json).map_err(failed)?;
        if let Some(url) = input["url"].as_str().filter(|s| s.len() <= 2048)
            && !sources.iter().any(|source: &Value| source["url"] == url)
        {
            sources.push(json!({"url":url,"operationId":op.id}));
            if sources.len() == 16 {
                break;
            }
        }
    }
    let operations: Vec<Value> = operations.into_iter().filter(|operation| operation.run_id.as_deref()==Some(run.id.as_str()))
        .rev().take(16).map(|operation| json!({
            "operationId":operation.id,"tool":operation.tool,"status":operation.evidence_status(),
            "inputDigest":operation.input_digest,"automaticReplay":"never",
            "argumentsPreview":operation.input_json.chars().take(2048).collect::<String>(),
            "humanEvidencePreview":operation.evidence.map(|text|text.chars().take(512).collect::<String>())
        })).collect();
    let verification = jeeves_state::verification::Verifications::new(server.pool.clone())
        .latest(&task.id)
        .await
        .map_err(failed)?;
    let verification=verification.map(|receipt| -> Result<Value,ErrorObject> {
        let files:Value=serde_json::from_str(&receipt.files_json).map_err(failed)?;
        Ok(json!({"receiptId":receipt.id,"runId":receipt.run_id,"roundId":receipt.round_id,
            "measuredAt":receipt.measured_at,"previousReceiptId":receipt.previous_id,"files":files}))
    }).transpose()?;
    let input = crate::task::inputs::prompt(server, &task.id, &run.id)
        .await
        .map_err(failed)?;
    let events = server.events.list(&task.id, 64).await.map_err(failed)?;
    let latest_edit_check: Option<String> = sqlx::query_scalar("SELECT detail FROM task_events WHERE task_id = ? AND kind = 'edit_check' ORDER BY seq DESC LIMIT 1")
        .bind(&task.id).fetch_optional(&server.pool).await.map_err(failed)?.flatten();
    let recent_events: Vec<Value> = events
        .into_iter()
        .take(16)
        .map(|event| {
            json!({
                "kind":event.kind,"detail":event.detail.map(|text| text.chars().take(256).collect::<String>()),"runId":event.run_id,"at":event.at
            })
        })
        .collect();
    let assignment: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT message_id,instruction,context,evidence_json FROM conversation_handoffs WHERE task_id=?",
    )
    .bind(&task.id)
    .fetch_optional(&server.pool)
    .await
    .map_err(failed)?;
    let assignment = assignment.map(|(message,instruction,context,evidence)| -> Result<Value,ErrorObject> {
        let operations:Vec<String> = serde_json::from_str(&evidence).map_err(failed)?;
        Ok(json!({"messageId":message,"instruction":instruction,"quotedContext":context,"settledOperationRefs":operations}))
    }).transpose()?;
    let amendments:Vec<(String,String)> = sqlx::query_as("SELECT message_id,instruction FROM task_amendments WHERE task_id=? ORDER BY rowid DESC LIMIT 16")
        .bind(&task.id).fetch_all(&server.pool).await.map_err(failed)?;
    let handoff = json!({
        "originalAssignment": assignment,
        "humanAmendmentsNewestFirst":amendments.into_iter().map(|(message,instruction)|json!({"messageId":message,"instruction":instruction})).collect::<Vec<_>>(),
        "schemaVersion":1,
        "externalInput":input,
        "recentOperationEvidenceNewestFirst":operations,
        "sourceReferences":sources,
        "recentTaskEventsNewestFirst":recent_events,
        "latestEditCheck":latest_edit_check.map(|text| text.chars().take(512).collect::<String>()),
        "pathsMentionedByEditOperationsNotRevalidated":edited.into_iter().take(50).collect::<Vec<_>>(),
        "lastRecordedOpenTodos":open_todos,
        "taskId":task.id,"runId":run.id,
        "contract":{"title":task.title,"goal":task.goal_condition,"acceptance":acceptance,"budget":budget,"cwd":task.cwd},
        "spent":{"tokens":task.spent_tokens,"tools":task.spent_tools,"wallMs":task.spent_wall_ms},
        "rounds":{"used":task.rounds_used,"limit":task.max_rounds},
        "plan":plan,
        "previousMeasurements":previous_checks,
        "historicalVerificationReceipt":verification,
        "recentStepsNewestFirst":recent_steps,
        "recordedArtifactsNotRevalidated":artifacts,
        "previousStopReason":task.last_stop_reason,
        "nextAction":"Continue the authorized contract from checkpointed progress. Recheck current artifacts and acceptance. Unknown effects require reconciliation, never an automatic repeat."
    });
    let text = serde_json::to_string(&handoff).map_err(failed)?;
    if text.len() > 64_000 {
        return Err(failed(
            "handoff exceeds 64000 bytes; shorten the contract or review its evidence",
        ));
    }
    Ok(format!(
        "\nDurable task handoff (schema 1, reconstructed from persistent records):\n{text}\nThe contract is independent of rolling summaries. Prior measurements and recorded artifacts are historical evidence, not current verification. Ledger operation keys do not prove external idempotency. This context never grants additional tool permissions.\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn missing_constraints_or_foreign_execution_context_are_rejected() {
        let home = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap();
        let server = Server::boot(
            pool.clone(),
            jeeves_config::Settings {
                home: home.path().into(),
                ..Default::default()
            },
        )
        .await;
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: "task",
                session_id: Some("owner"),
                title: "contract",
                goal_condition: None,
                goal_draft: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
        server
            .runs
            .start("run", "task", Some("work"), Some("execution"))
            .await
            .unwrap();
        assert!(load(&server, "execution", "another-work").await.is_err());
        assert!(
            load(&server, "ordinary-chat", "owner")
                .await
                .unwrap()
                .is_empty()
        );
        let before = load(&server, "execution", "work").await.unwrap();
        assert!(before.contains("Durable task handoff"));
        server
            .events
            .append(
                "task",
                Some("run"),
                "edit_check",
                Some("[check build] exit 1"),
            )
            .await
            .unwrap();
        for (id, tool, input) in [
            ("edit-op", "edit", r#"{"path":"src/lib.rs"}"#),
            (
                "todo-op",
                "todo_write",
                r#"{"todos":[{"content":"finish tests","status":"pending"}]}"#,
            ),
        ] {
            sqlx::query("INSERT INTO tool_operations(id,task_id,run_id,turn_id,session_id,round_id,tool_use_id,tool,input_json,input_digest,status,output_preview) VALUES(?, 'task', 'run', 'execution', 'work', 'round', ?, ?, ?, 'digest', 'succeeded', 'recorded')")
                .bind(id).bind(id).bind(tool).bind(input).execute(&pool).await.unwrap();
        }
        let restored = load(&server, "execution", "work").await.unwrap();
        assert!(restored.contains("[check build] exit 1"));
        assert!(restored.contains("src/lib.rs"));
        assert!(restored.contains("finish tests"));
        server
            .runs
            .start("ambiguous", "task", Some("work"), Some("execution"))
            .await
            .unwrap();
        assert!(load(&server, "execution", "work").await.is_err());
        sqlx::query("DELETE FROM task_runs WHERE id='ambiguous'")
            .execute(&pool)
            .await
            .unwrap();
        for (acceptance, budget) in [
            ("null", "{}"),
            (r#"[{"kind":"fileExist","path":"report.md"}]"#, "{}"),
            ("[]", r#"{"tokens":-1}"#),
            ("[]", r#"{"tokens":"ten"}"#),
        ] {
            sqlx::query("UPDATE agent_tasks SET acceptance_json=?,budget_json=? WHERE id='task'")
                .bind(acceptance)
                .bind(budget)
                .execute(&pool)
                .await
                .unwrap();
            assert!(
                load(&server, "execution", "work").await.is_err(),
                "invalid contract: {acceptance} {budget}"
            );
        }
        sqlx::query("UPDATE agent_tasks SET acceptance_json='invalid JSON' WHERE id='task'")
            .execute(&pool)
            .await
            .unwrap();
        let error = load(&server, "execution", "work").await.unwrap_err();
        assert!(error.message.contains("execution stopped"));
    }
}
