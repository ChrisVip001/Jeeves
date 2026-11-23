//! Call one approved read-only tool with fixed arguments and return its result as text.
use super::{
    Server,
    action::{Adapter, Output},
    trigger,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Fetch {
    pub(super) tool: String,
    pub(super) args: Value,
}

pub(super) fn config(value: &Value) -> Result<Fetch> {
    let config: Fetch = serde_json::from_value(value.clone())?;
    ensure!(trigger::slug(&config.tool), "Choose a valid tool name");
    ensure!(config.args.is_object(), "Tool arguments must be an object");
    ensure!(
        config.args.to_string().len() <= 4096,
        "Tool arguments are too large"
    );
    Ok(config)
}

pub(super) fn validate(server: &Server, source: &Fetch) -> Result<()> {
    let tools = registry(server);
    ensure!(
        tools.effect(&source.tool).is_read(),
        "Tool {} is unavailable or not read-only",
        source.tool
    );
    let tool = tools
        .get(&source.tool)
        .with_context(|| format!("Tool {} is not installed", source.tool))?;
    jeeves_tools::validate_value(&tool.input_schema, &source.args)
        .map_err(|error| anyhow::anyhow!("Tool arguments are invalid: {error}"))
}

pub(super) async fn read(server: &Server, wake_key: &str, source: &Fetch) -> Result<Value> {
    let tools = registry(server);
    ensure!(
        tools.effect(&source.tool).is_read(),
        "Tool {} is no longer read-only",
        source.tool
    );
    let result = tools
        .execute_result(
            server.tool_ctx(wake_key, None, None),
            &source.tool,
            source.args.clone(),
        )
        .await;
    ensure!(
        result.success,
        "Source {} failed: {}",
        source.tool,
        result.text
    );
    serde_json::from_str(&result.text)
        .with_context(|| format!("Source {} is not structured", source.tool))
}

fn registry(server: &Server) -> jeeves_tools::ToolRegistry {
    let settings = server.settings();
    crate::enabled_tools(
        &crate::connector_settings::apply(&settings.home, &server.tools),
        &settings,
    )
}

pub(super) fn catalog(server: &Server) -> Value {
    let tools = registry(server);
    Value::Array(
        tools
            .names()
            .into_iter()
            .filter(|name| tools.effect(name).is_read())
            .filter_map(|name| {
                tools.get(name).map(|tool| {
                    json!({
                        "tool":name,
                        "description":tool.description,
                        "inputSchema":tool.input_schema,
                    })
                })
            })
            .collect(),
    )
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "fetch",
    label: "Read from an approved source",
    uses_model: false,
    initial_tools: 1,
    check_source: None,
    schema: || {
        json!({"type":"object","required":["tool","args"],"additionalProperties":false,"properties":{
            "tool":{"type":"string"},"args":{"type":"object"}
        }})
    },
    describe: |value| {
        let config = config(value)?;
        Ok(vec![format!("{} {}", config.tool, config.args)])
    },
    preflight: |server, value| validate(server, &config(value)?),
    run: |input, value| {
        Box::pin(async move {
            let source = config(value)?;
            let record = read(input.server, input.wake_key, &source).await?;
            let mut text = serde_json::to_string(&record)?;
            if text.len() > 32_000 {
                text.truncate(32_000);
                text.push('…');
            }
            Ok(Output {
                body: Some(text),
                tools_used: 1,
            })
        })
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_config_requires_a_named_tool_and_object_arguments() {
        assert!(config(&json!({"tool":"calendar_list","args":{}})).is_ok());
        assert!(config(&json!({"tool":"Bad Name","args":{}})).is_err());
        assert!(config(&json!({"tool":"calendar_list","args":[]})).is_err());
        assert!(config(&json!({"tool":"calendar_list"})).is_err());
    }

    #[test]
    fn fetch_config_rejects_unknown_fields() {
        assert!(config(&json!({"tool":"calendar_list","args":{},"extra":1})).is_err());
    }
}
