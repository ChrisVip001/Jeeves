//! Host side of `jeeves_tools::edit_checks::EditChecks`: run a task's `after: edit` checks in its
//! worktree through the same exec gate and sandbox as every other check.
use crate::Server;
use jeeves_agent::verify::{CommandOutcome, CommandRunner};
use jeeves_tools::edit_checks::{CheckFut, EditChecks};
use std::path::PathBuf;
use std::time::Duration;

const OUTPUT_LIMIT: usize = 4096;
const EDIT_CHECK_TIMEOUT: Duration = Duration::from_secs(60);

pub(crate) struct HostEditChecks(pub(crate) Server, pub(crate) jeeves_tools::ToolCtx);

/// At most `OUTPUT_LIMIT` bytes, cut on a character boundary, with an honest marker.
fn clip(text: &str) -> String {
    let text = text.trim();
    if text.len() <= OUTPUT_LIMIT {
        return text.to_string();
    }
    let mut end = OUTPUT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (output cut at {OUTPUT_LIMIT} bytes)", &text[..end])
}

pub(crate) fn render(name: &str, outcome: Option<CommandOutcome>) -> String {
    match outcome {
        None => format!(
            "[check {name}] not finished within {}s",
            EDIT_CHECK_TIMEOUT.as_secs()
        ),
        Some(CommandOutcome::Skipped(why)) => format!("[check {name}] not run: {why}"),
        Some(CommandOutcome::Ran { code: 0, .. }) => format!("[check {name}] passed"),
        Some(CommandOutcome::Ran { code, output }) => {
            format!("[check {name}] exit {code}\n{}", clip(&output))
        }
    }
}

impl EditChecks for HostEditChecks {
    fn after_edit(
        &self,
        run_id: Option<String>,
        cwd: Option<PathBuf>,
        lease: Option<jeeves_tools::resources::Lease>,
    ) -> CheckFut {
        let server = self.0.clone();
        let mut ctx = self.1.clone();
        ctx.resource_lease = lease;
        Box::pin(async move {
            let run = server.runs.get(&run_id?).await.ok()??;
            let json = jeeves_state::EditChecks::new(server.pool.clone())
                .get(&run.task_id)
                .await
                .ok()??;
            let checks: Vec<jeeves_protocol::CodeCheckSpec> = serde_json::from_str(&json).ok()?;
            let cwd = cwd?;
            let task = match server.tasks.get(&run.task_id).await {
                Ok(Some(task)) => task,
                _ => return Some("[check] not run: task workspace unavailable".into()),
            };
            let runner = server.check_runner(
                ctx,
                None,
                crate::task::worktree::owned(&server.settings().home, Some(&task)),
            );
            let mut lines = Vec::new();
            for check in checks {
                let outcome = tokio::time::timeout(
                    EDIT_CHECK_TIMEOUT,
                    runner.run(check.argv.clone(), cwd.clone()),
                )
                .await
                .ok();
                let result = render(&check.name, outcome);
                let summary = result.lines().next().unwrap_or("check result unavailable");
                if let Err(error) = server
                    .events
                    .append(&run.task_id, Some(&run.id), "edit_check", Some(summary))
                    .await
                {
                    eprintln!("(jeeves) could not record edit check: {error}");
                }
                lines.push(result);
            }
            (!lines.is_empty()).then(|| lines.join("\n"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_render_as_diagnostics_the_model_can_act_on() {
        assert_eq!(
            render(
                "t",
                Some(CommandOutcome::Ran {
                    code: 0,
                    output: String::new()
                })
            ),
            "[check t] passed"
        );
        let failed = render(
            "t",
            Some(CommandOutcome::Ran {
                code: 1,
                output: "error[E0308]".into(),
            }),
        );
        assert!(failed.starts_with("[check t] exit 1\n") && failed.contains("E0308"));
        assert_eq!(
            render("t", Some(CommandOutcome::Skipped("exec off".into()))),
            "[check t] not run: exec off"
        );
        assert!(render("t", None).contains("not finished within"));
        let long = render(
            "t",
            Some(CommandOutcome::Ran {
                code: 2,
                output: "é".repeat(5000),
            }),
        );
        assert!(long.len() < OUTPUT_LIMIT + 100 && long.contains("output cut"));
    }

    #[tokio::test]
    async fn a_task_with_checks_gets_diagnostics_and_a_final_acceptance_condition() {
        use crate::input_wake_tests::{fixture, rpc};
        let (_home, server, _model) = fixture().await;
        let made = rpc(
            &server,
            "task/create",
            serde_json::json!({"title":"fix","codeChecks":[
                {"name":"build","argv":["cargo","check"],"after":"edit"},
                {"name":"tests","argv":["cargo","test"],"after":"final"}]}),
        )
        .await
        .unwrap();
        let id = made["task"]["id"].as_str().unwrap().to_string();
        let acceptance = made["task"]["acceptance"].as_array().unwrap();
        assert_eq!(
            acceptance.len(),
            1,
            "only the final check is an acceptance condition"
        );
        assert_eq!(acceptance[0]["kind"], "commandSucceeds");
        server
            .runs
            .start("turn#1", &id, None, Some("turn"))
            .await
            .unwrap();
        let hook = HostEditChecks(server.clone(), server.tool_ctx("default", None, None));
        let out = hook
            .after_edit(Some("turn#1".into()), Some(std::env::temp_dir()), None)
            .await
            .expect("the task declared an edit check");
        assert_eq!(
            out, "[check build] not run: exec off",
            "exec is off in the fixture"
        );
        let events = server.events.list(&id, 20).await.unwrap();
        assert!(events.iter().any(|event| event.kind == "edit_check"
            && event.detail.as_deref() == Some("[check build] not run: exec off")));
        assert!(hook
            .after_edit(Some("no-such-run".into()), None, None)
            .await
            .is_none());
        let e = rpc(&server, "task/create", serde_json::json!({"title":"x","codeChecks":[{"name":"c","argv":["a"],"after":"never"}]}))
            .await
            .unwrap_err();
        assert!(e.contains("after must be"), "{e}");
    }
}
