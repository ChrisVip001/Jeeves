//! Human-owned MCP switches. Effects always come from the connection manifest.
use jeeves_mcp::config::{Config, ServerSpec};
use jeeves_protocol::{ConnectorToolView, ConnectorToolsResponse, ConnectorToolsSetParams};
use std::collections::BTreeMap;
use std::path::Path;

fn tools(
    spec: &ServerSpec,
    root: &Path,
    remote: &[String],
) -> Result<Vec<ConnectorToolView>, String> {
    let rules = spec
        .effective_manifest(root)?
        .map(|m| m.tools)
        .unwrap_or_default();
    let mut rows = BTreeMap::new();
    for name in remote.iter().chain(rules.keys()) {
        let rule = rules.get(name);
        rows.insert(
            name,
            ConnectorToolView {
                name: name.clone(),
                effect: rule.map_or_default(|r| r.effect),
                disabled: rule.is_some_and(|r| r.disabled),
            },
        );
    }
    Ok(rows.into_values().collect())
}

impl crate::Server {
    pub(crate) async fn connector_tools(&self, id: &str) -> Result<ConnectorToolsResponse, String> {
        let name = id
            .strip_prefix("mcp:")
            .ok_or("expected an MCP capability id")?;
        let home = self.settings().home;
        self.mcp.refresh(&home).await;
        let remote = self.mcp.remote_tool_names(name);
        for path in crate::server::mcp_live::config_paths(&home) {
            if !path.exists() {
                continue;
            }
            if let Some(spec) = Config::read(&path)?
                .servers
                .into_iter()
                .find(|s| s.name == name)
            {
                return Ok(ConnectorToolsResponse {
                    tools: tools(&spec, path.parent().unwrap_or(&home), &remote)?,
                    editable: path == home.join("mcp.json"),
                });
            }
        }
        Err("MCP connector no longer configured".into())
    }

    pub(crate) async fn set_connector_tool(
        &self,
        p: ConnectorToolsSetParams,
    ) -> Result<ConnectorToolsResponse, String> {
        let name =
            p.id.strip_prefix("mcp:")
                .ok_or("expected an MCP capability id")?;
        let home = self.settings().home;
        let remote = self.mcp.remote_tool_names(name);
        jeeves_mcp::config::edit(&home, |cfg| {
            let spec = cfg
                .servers
                .iter_mut()
                .find(|s| s.name == name)
                .ok_or("only user connections can be edited")?;
            if !tools(spec, &home, &remote)?
                .iter()
                .any(|r| r.name == p.tool)
            {
                return Err("unknown connector tool".into());
            }
            let declared = spec.manifest.as_ref().map(|s| s.load(&home)).transpose()?;
            let default_disabled = declared
                .as_ref()
                .and_then(|m| m.tools.get(&p.tool))
                .is_some_and(|r| r.disabled);
            if p.disabled == default_disabled {
                spec.tool_switches.remove(&p.tool);
            } else {
                spec.tool_switches.insert(p.tool.clone(), !p.disabled);
            }
            spec.effective_manifest(&home)?;
            Ok(())
        })?;
        self.capabilities.invalidate();
        eprintln!("(jeeves) connector tool switch saved for {name}; effective on the next turn");
        self.connector_tools(&p.id).await
    }
}
