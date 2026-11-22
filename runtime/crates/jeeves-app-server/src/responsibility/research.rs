//! Verify a summary from its saved research workspace and bounded source identities.
use super::action::{Adapter, Output};
use anyhow::{ensure, Context};
use serde::Deserialize;
use serde_json::json;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Research {
    task_id: String,
    sources: Vec<String>,
    lookback_days: u32,
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "verifiedSummary",
    label: "Verified source summary",
    uses_model: false,
    initial_tools: 1,
    check_source: None,
    schema: || {
        json!({"type":"object","required":["taskId","sources","lookbackDays"],"properties":{
        "taskId":{"type":"string"},"sources":{"type":"array","items":{"type":"string"}},"lookbackDays":{"type":"integer","minimum":1,"maximum":90}
    },"additionalProperties":false})
    },
    describe: |value| {
        let Research {
            task_id,
            sources,
            lookback_days,
        } = serde_json::from_value(value.clone())?;
        ensure!(!task_id.trim().is_empty(), "Select a saved research task");
        super::validate_research_sources(sources.iter().map(String::as_str))?;
        ensure!(
            (1..=90).contains(&lookback_days),
            "Choose a source window of 1 to 90 days"
        );
        Ok(vec![
            task_id,
            sources.join(", "),
            format!("{lookback_days} days"),
        ])
    },
    preflight: |_, _| Ok(()),
    run: |input, value| {
        Box::pin(async move {
            let server = input.server;
            let Research {
                task_id,
                sources,
                lookback_days,
            } = serde_json::from_value(value.clone())?;
            let at = input.at;
            let root = server
                .tasks
                .get(&task_id)
                .await?
                .and_then(|task| task.cwd)
                .context("Research task or its workspace is unavailable")?;
            let body = tokio::task::spawn_blocking(move || {
                jeeves_agent::verify_sources::verified_summary(
                    &PathBuf::from(root),
                    &sources,
                    at,
                    lookback_days,
                )
            })
            .await?
            .map_err(anyhow::Error::msg)?;
            Ok(Output {
                body: Some(body),
                tools_used: 1,
            })
        })
    },
};
