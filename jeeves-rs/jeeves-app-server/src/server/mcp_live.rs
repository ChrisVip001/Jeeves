//! The MCP half of the tool table, rebuilt between turns when `mcp.json` changes.
//!
//! A turn snapshots its table when it starts, so a rebuild never changes a turn in flight.
//! Rebuilds are announced on stderr with the tools added and removed; they are never silent.

use jeeves_mcp::McpClient;
use jeeves_tools::ToolRegistry;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub(crate) struct McpLive {
    client: Mutex<Arc<McpClient>>,
    /// Only the MCP names present in the original, immutable base table.
    base_names: Vec<String>,
    fingerprint: Mutex<String>,
    refresh_lock: tokio::sync::Mutex<()>,
    force_refresh: std::sync::atomic::AtomicBool,
}

/// What the config files say right now (content, so an edit that keeps the mtime still counts).
pub(crate) fn fingerprint(home: &Path) -> String {
    config_paths(home)
        .iter()
        .map(|path| {
            let mut contents = std::fs::read_to_string(path).unwrap_or_default();
            if let Ok(config) = jeeves_mcp::config::Config::parse(&contents) {
                for spec in config.servers {
                    if let Some(file @ jeeves_mcp::manifest::ManifestSource::File(_)) =
                        spec.manifest
                    {
                        let current = file.load(path.parent().unwrap_or(home));
                        contents.push('\0');
                        contents.push_str(
                            &serde_json::to_string(&current).expect("serializable manifest result"),
                        );
                    }
                }
            }
            contents
        })
        .collect::<Vec<_>>()
        .join("\u{0}")
}

pub(crate) fn config_paths(home: &Path) -> Vec<std::path::PathBuf> {
    let extra = std::env::var_os("JEEVES_MCP_CONFIG");
    jeeves_mcp::config::config_paths_for(home, extra.as_deref())
}

/// `(added, removed)` between two name lists, each sorted.
pub(crate) fn diff(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let mut added: Vec<String> = new.iter().filter(|n| !old.contains(n)).cloned().collect();
    let mut removed: Vec<String> = old.iter().filter(|n| !new.contains(n)).cloned().collect();
    added.sort();
    removed.sort();
    (added, removed)
}

impl McpLive {
    pub(crate) fn new(client: McpClient, home: &Path, base: &mut ToolRegistry) -> Self {
        let base_names = client.register_tools(base);
        Self {
            client: Mutex::new(Arc::new(client)),
            base_names,
            fingerprint: Mutex::new(fingerprint(home)),
            refresh_lock: tokio::sync::Mutex::new(()),
            force_refresh: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub(crate) fn remote_tool_names(&self, name: &str) -> Vec<String> {
        self.client
            .lock()
            .expect("mcp lock")
            .remote_tool_names(name)
    }

    pub(crate) fn servers(&self) -> Vec<String> {
        let client = self.client.lock().expect("mcp lock").clone();
        client.servers().into_iter().map(String::from).collect()
    }

    pub(crate) fn missing_scope(&self) -> Vec<String> {
        self.client
            .lock()
            .expect("mcp lock")
            .missing_scope()
            .to_vec()
    }

    pub(crate) fn invalidate(&self) {
        self.force_refresh
            .store(true, std::sync::atomic::Ordering::Release);
    }

    pub(crate) async fn close(&self) {
        let client = self.client.lock().expect("mcp lock").clone();
        client.close().await;
    }

    /// The base table with the MCP tools it was built with swapped for the current ones.
    pub(crate) fn apply(&self, base: &ToolRegistry) -> ToolRegistry {
        let mut tools = base.clone();
        for name in &self.base_names {
            tools = tools.without(name);
        }
        self.client
            .lock()
            .expect("mcp lock")
            .register_tools(&mut tools);
        tools
    }

    /// Reconnect if the config changed. Returns `(added, removed)` tool names when it did.
    pub(crate) async fn refresh(&self, home: &Path) -> Option<(Vec<String>, Vec<String>)> {
        let _guard = self.refresh_lock.lock().await;
        let now = fingerprint(home);
        let forced = self
            .force_refresh
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        let same = *self.fingerprint.lock().expect("mcp lock") == now && !forced;
        let next = if same {
            let current = self.client.lock().expect("mcp lock").clone();
            current.refresh_lists().await?
        } else {
            McpClient::connect_home(home).await
        };
        for warning in next.warnings() {
            eprintln!("(jeeves) {warning}");
        }
        let new_names: Vec<String> = next.tools().into_iter().map(|t| t.name).collect();
        let old = std::mem::replace(&mut *self.client.lock().expect("mcp lock"), Arc::new(next));
        let old_names: Vec<_> = old.tools().into_iter().map(|t| t.name).collect();
        *self.fingerprint.lock().expect("mcp lock") = now;
        Some(diff(&old_names, &new_names))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_lists_added_and_removed() {
        let old = vec!["a".to_string(), "b".to_string()];
        let new = vec!["b".to_string(), "c".to_string()];
        assert_eq!(
            diff(&old, &new),
            (vec!["c".to_string()], vec!["a".to_string()])
        );
    }

    #[tokio::test]
    async fn unchanged_config_does_not_rebuild_and_a_change_does() {
        let dir = tempfile::tempdir().unwrap();
        let live = McpLive::new(McpClient::default(), dir.path(), &mut ToolRegistry::new());
        assert!(live.refresh(dir.path()).await.is_none());
        std::fs::write(dir.path().join("mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
        let (added, removed) = live.refresh(dir.path()).await.expect("changed");
        assert!(added.is_empty() && removed.is_empty());
        assert!(live.refresh(dir.path()).await.is_none());
    }

    #[tokio::test]
    async fn config_change_consumes_a_pending_oauth_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let live = McpLive::new(McpClient::default(), dir.path(), &mut ToolRegistry::new());
        live.invalidate();
        std::fs::write(dir.path().join("mcp.json"), r#"{"servers":[]}"#).unwrap();
        assert!(live.refresh(dir.path()).await.is_some());
        assert!(live.refresh(dir.path()).await.is_none());
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    const FAKE: &str = r#"
import sys, json
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    method = m["method"]
    if method == "initialize":
        r = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}}, "serverInfo": {"name": "f", "version": "1"}}
    elif method == "tools/list":
        r = {"tools": [{"name": "ping", "description": "d", "inputSchema": {"type": "object", "properties": {}}}]}
    else:
        r = {}
    print(json.dumps({"jsonrpc": "2.0", "id": m["id"], "result": r}), flush=True)
"#;

    #[tokio::test]
    async fn dynamic_tool_list_changes_only_the_next_table() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("dynamic.py");
        std::fs::write(&script, r#"
import sys, json
n = 0
for line in sys.stdin:
    m = json.loads(line)
    if 'id' not in m: continue
    if m['method'] == 'initialize':
        r = {'protocolVersion':'2025-03-26','capabilities':{'tools':{'listChanged':True}},'serverInfo':{'name':'f','version':'1'}}
    elif m['method'] == 'tools/list':
        n += 1
        if n == 2:
            print(json.dumps({'jsonrpc':'2.0','id':m['id'],'error':{'code':-32000,'message':'temporary unavailable'}}),flush=True)
            continue
        r = {'tools':[{'name':'before' if n == 1 else 'after','inputSchema':{'type':'object'}}]}
        if n == 1: print(json.dumps({'jsonrpc':'2.0','method':'notifications/tools/list_changed'}), flush=True)
    else: r = {'content':[{'type':'text','text':'ok'}]}
    print(json.dumps({'jsonrpc':'2.0','id':m['id'],'result':r}),flush=True)
"#).unwrap();
        std::fs::write(
            dir.path().join("mcp.json"),
            serde_json::json!({"servers":[{"name":"fake","command":"python3","args":[script]}]})
                .to_string(),
        )
        .unwrap();
        let client = McpClient::connect_home(dir.path()).await;
        let mut base = ToolRegistry::new();
        let live = McpLive::new(client, dir.path(), &mut base);
        let before = live.apply(&base);
        assert!(
            live.refresh(dir.path()).await.is_none(),
            "a failed refresh retains the previous list"
        );
        assert_eq!(live.apply(&base).names(), vec!["fake_before"]);
        let (added, removed) = live.refresh(dir.path()).await.expect("changed list");
        assert_eq!(added, vec!["fake_after"]);
        assert_eq!(removed, vec!["fake_before"]);
        assert_eq!(before.names(), vec!["fake_before"]);
        assert_eq!(live.apply(&base).names(), vec!["fake_after"]);
        live.close().await;
    }

    #[tokio::test]
    async fn a_user_switch_changes_registration_without_reclassifying_unknown_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::write(home.join("server.py"), FAKE).unwrap();
        std::fs::write(home.join("mcp.json"),serde_json::json!({"servers":[{"name":"demo","command":"python3","args":[home.join("server.py")]}]}).to_string()).unwrap();
        let mut base = ToolRegistry::new();
        let live = McpLive::new(McpClient::connect_home(home).await, home, &mut base);
        assert!(base.get("demo_ping").is_some());
        for enabled in [false, true] {
            jeeves_mcp::config::edit(home, |cfg| {
                cfg.servers[0].tool_switches.insert("ping".into(), enabled);
                Ok(())
            })
            .unwrap();
            live.refresh(home).await.unwrap();
            let table = live.apply(&base);
            assert_eq!(table.get("demo_ping").is_some(), enabled);
            assert_eq!(table.effect("demo_ping"), jeeves_tools::ToolEffect::Write);
        }
        live.close().await;
    }

    #[tokio::test]
    async fn sidecar_changes_reclassify_or_remove_tools_on_the_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(home.join("fake.py"), FAKE).unwrap();
        std::fs::write(home.join("mcp.json"), serde_json::json!({"servers": [{
            "name":"fake","command":"python3","args":[home.join("fake.py")],"manifest":"connector.json"
        }]}).to_string()).unwrap();
        let sidecar = home.join("connector.json");
        std::fs::write(&sidecar, r#"{"category":"other","tools":{"ping":{}}}"#).unwrap();
        let mut base = ToolRegistry::new();
        let live = McpLive::new(McpClient::connect_home(home).await, home, &mut base);
        use jeeves_tools::ToolEffect;
        assert_eq!(base.effect("fake_ping"), ToolEffect::Write);
        assert!(base.get("fake_ping").is_some());
        std::fs::write(
            &sidecar,
            r#"{"category":"other","tools":{"ping":{"effect":"read"}}}"#,
        )
        .unwrap();
        assert!(live.refresh(home).await.is_some());
        assert_eq!(live.apply(&base).effect("fake_ping"), ToolEffect::Read);
        for invalid_or_disabled in [
            r#"{"category":"other","tools":{"ping":{"disabled":true}}}"#,
            r#"{"category":"other","verbs":{"tasks.list":"ping"}}"#,
        ] {
            std::fs::write(&sidecar, invalid_or_disabled).unwrap();
            assert!(live.refresh(home).await.is_some());
            assert!(live.apply(&base).get("fake_ping").is_none());
        }
        std::fs::remove_file(sidecar).unwrap();
        assert!(live.refresh(home).await.is_some());
        assert!(live.apply(&base).get("fake_ping").is_none());
        live.close().await;
    }

    #[tokio::test]
    async fn a_server_added_by_config_shows_up_for_the_next_table_only() {
        if std::process::Command::new("python3")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake.py");
        std::fs::write(&script, FAKE).unwrap();
        let live = McpLive::new(McpClient::default(), dir.path(), &mut ToolRegistry::new());
        let base = ToolRegistry::new();
        let before = live.apply(&base);
        std::fs::write(
            dir.path().join("mcp.json"),
            serde_json::json!({"servers": [{"name": "fake", "command": "python3", "args": [script]}]})
                .to_string(),
        )
        .unwrap();
        let (added, removed) = live.refresh(dir.path()).await.expect("changed");
        assert_eq!(added.len(), 1, "{added:?}");
        assert!(removed.is_empty());
        assert!(live.apply(&base).names().len() == 1);
        assert!(before.names().is_empty(), "the held table is unchanged");
        live.close().await;
    }
}
