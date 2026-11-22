//! Verify a report from its saved research workspace and bounded source identities.
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
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "researchDigest",
    label: "Verified research report",
    uses_model: false,
    schema: || {
        json!({"type":"object","required":["taskId","sources"],"properties":{
        "taskId":{"type":"string"},"sources":{"type":"array","items":{"type":"string"}}
    },"additionalProperties":false})
    },
    describe: |value| {
        let Research { task_id, sources } = serde_json::from_value(value.clone())?;
        ensure!(!task_id.trim().is_empty(), "Select a saved research task");
        super::validate_research_sources(sources.iter().map(String::as_str))?;
        Ok(vec![task_id, sources.join(", ")])
    },
    preflight: |_, _| Ok(()),
    run: |input, value| {
        Box::pin(async move {
            let server = input.server;
            let Research { task_id, sources } = serde_json::from_value(value.clone())?;
            let at = input.at;
            let root = server
                .tasks
                .get(&task_id)
                .await?
                .and_then(|task| task.cwd)
                .context("Research task or its workspace is unavailable")?;
            let body = tokio::task::spawn_blocking(move || {
                jeeves_agent::verify_sources::weekly_report(&PathBuf::from(root), &sources, at)
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
