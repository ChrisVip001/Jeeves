//! Only the owner's MCP configuration can declare a tool's effect. A peer's annotations cannot.
use jeeves_protocol::{ConnectorCatalogResponse, ConnectorCategory, ConnectorManifest, ToolEffect};
use serde::{Deserialize, Serialize};
use std::{
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ManifestSource {
    Inline(ConnectorManifest),
    File(String),
}

impl ManifestSource {
    pub fn path(&self, root: &Path) -> Result<Option<PathBuf>, String> {
        let Self::File(name) = self else {
            return Ok(None);
        };
        let file = Path::new(name);
        if file.file_name().and_then(|n| n.to_str()) != Some("connector.json")
            || !file.components().all(|c| matches!(c, Component::Normal(_)))
        {
            return Err(
                "manifest must name a relative connector.json under the MCP config directory"
                    .into(),
            );
        }
        Ok(Some(root.join(file)))
    }

    pub fn load(&self, root: &Path) -> Result<ConnectorManifest, String> {
        let manifest = match self {
            Self::Inline(manifest) => manifest.clone(),
            Self::File(_) => {
                let path = self.path(root)?.expect("file manifest");
                let real = path
                    .canonicalize()
                    .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
                if !real.starts_with(root.canonicalize().map_err(|e| e.to_string())?) {
                    return Err(
                        "connector manifest resolved outside its configuration directory".into(),
                    );
                }
                let (text, _) = jeeves_state::verification::FileMeasurement::read_text_limited(
                    &real,
                    128 * 1024,
                )
                .map_err(|e| format!("cannot read connector manifest: {e}"))?;
                serde_json::from_str(&text).map_err(|e| format!("invalid connector.json: {e}"))?
            }
        };
        validate(&manifest)?;
        Ok(manifest)
    }
}

pub fn validate(manifest: &ConnectorManifest) -> Result<(), String> {
    use ConnectorCategory::{Calendar, Mail, Tasks};
    use ToolEffect::{Draft, Read, Send};
    for (name, rule) in &manifest.tools {
        if name.is_empty()
            || name.len() > 128
            || name.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("connector tool needs a nonempty remote name of at most 128 bytes".into());
        }
        if rule.resource.as_ref().is_some_and(|s| {
            s.trim().is_empty() || s.len() > 256 || s.chars().any(char::is_control)
        }) {
            return Err(format!("invalid resource scope for {name}"));
        }
    }
    for (verb, name) in &manifest.verbs {
        let (category, effect) = match verb.as_str() {
            "calendar.list_events" => (Calendar, Read),
            "calendar.create_draft" => (Calendar, Draft),
            "mail.search" => (Mail, Read),
            "mail.draft_reply" => (Mail, Draft),
            "mail.send" => (Mail, Send),
            "tasks.list" => (Tasks, Read),
            _ => return Err(format!("unknown standard verb {verb}")),
        };
        if manifest.category != category
            || !manifest.tools.get(name).is_some_and(|r| r.effect == effect)
        {
            return Err(format!(
                "{verb} requires a declared {effect:?} tool in category {category:?}"
            ));
        }
    }
    Ok(())
}

pub fn catalog() -> &'static ConnectorCatalogResponse {
    static CATALOG: OnceLock<ConnectorCatalogResponse> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../catalog/catalog.json"))
            .expect("checked bundled connector catalog")
    })
}

pub fn builtin(id: &str) -> &'static ConnectorManifest {
    &catalog()
        .entries
        .iter()
        .find(|entry| entry.id == id && entry.setup == jeeves_protocol::ConnectorSetup::Builtin)
        .expect("checked native connector manifest")
        .manifest
}

pub(crate) async fn approve(
    ctx: &jeeves_tools::ToolCtx,
    server: &str,
    name: &str,
    effect: ToolEffect,
    scope: Option<&str>,
    args: &serde_json::Value,
) -> anyhow::Result<()> {
    use jeeves_tools::approval::{ActionGrant, ApprovalRequest};
    let broker = ctx
        .approval
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("MCP {server}/{name} requires human approval"))?;
    let preview = jeeves_tools::permissions::redact(args.clone()).to_string();
    anyhow::ensure!(
        preview.len() <= 8192,
        "MCP argument preview is too large for approval"
    );
    let digest = jeeves_state::ledger::digest_of(args.to_string().as_bytes());
    let resource = scope.unwrap_or(name);
    let request = ApprovalRequest {
        tool: format!("{server}/{name}"),
        args_preview: format!(
            "{effect:?} · {server}/{name} · {resource}\n{preview}\nArgument digest: {digest}"
        ),
        reason: "This connector operation requires one approval".into(),
        timeout_secs: 120,
        remember: None,
        action: Some(ActionGrant {
            account: format!("mcp:{server}"),
            resource: resource.into(),
            action: name.into(),
            destination: format!("{server}/{name}"),
            content_hash: Some(digest),
            scope: "once".into(),
            expires_at: None,
        }),
    };
    let outcome = broker.request(request).await;
    anyhow::ensure!(outcome.approved(), "{}", outcome.refusal("MCP operation"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn shipped_catalog_has_distinct_ids_sources_and_valid_manifests() {
        let entries = &catalog().entries;
        assert_eq!(entries.len(), 20);
        let mut ids = std::collections::HashSet::new();
        for entry in entries {
            assert!(ids.insert(&entry.id));
            assert!(entry.documentation_url.starts_with("https://"));
            assert_eq!(
                entry.endpoint.is_none(),
                entry.setup == jeeves_protocol::ConnectorSetup::Builtin
            );
            if let Some(endpoint) = &entry.endpoint {
                assert!(endpoint.starts_with("https://"));
            }
            validate(&entry.manifest).unwrap();
        }
    }
    #[test]
    fn malformed_or_misclassified_verb_rejects_the_entire_manifest() {
        for fields in [
            json!({"category":"mail","verbs":{"mail.send":"lookup"},"tools":{"lookup":{"effect":"read"}}}),
            json!({"category":"calendar","verbs":{"tasks.list":"query"},"tools":{"query":{"effect":"read"}}}),
            json!({"category":"mail","tools":{"q":{"efect":"read"}}}),
            json!({"category":"other","verbs":{"custom.foo":"missing"}}),
        ] {
            assert!(serde_json::from_value::<ManifestSource>(fields)
                .and_then(|source| source
                    .load(Path::new("."))
                    .map_err(serde::de::Error::custom))
                .is_err());
        }
        let spec: ConnectorManifest =
            serde_json::from_value(json!({"category":"tasks","tools":{"x":{}}})).unwrap();
        assert_eq!(spec.tools["x"].effect, ToolEffect::Write);
    }
}
