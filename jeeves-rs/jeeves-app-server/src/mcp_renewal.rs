//! Idle, resident credential maintenance. No turn or business tool is dispatched here.
use jeeves_mcp::{config::Config, oauth::TokenStore, renewal::refresh};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

pub(crate) async fn run(server: crate::Server) {
    let home = server.settings().home;
    let mut failures = HashMap::new();
    loop {
        let mut current = HashMap::new();
        let mut seen = HashSet::new();
        for path in crate::server::mcp_live::config_paths(&home) {
            if !path.exists() {
                continue;
            }
            let Ok(config) = Config::read(&path) else {
                current.insert(
                    path.display().to_string(),
                    "unreadable MCP configuration".to_string(),
                );
                continue;
            };
            for spec in config.servers {
                if !seen.insert(spec.name.clone())
                    || spec.oauth != Some(true)
                    || spec.command.is_some()
                {
                    continue;
                }
                let Some(url) = spec.url else { continue };
                let store = TokenStore::new(path.parent().unwrap_or(&home), &spec.name);
                match refresh(&store, &url, true).await {
                    Ok(true) => {
                        eprintln!("(jeeves) OAuth renewed for {}", spec.name);
                        server.capabilities.invalidate();
                        if !server.mcp.servers().contains(&spec.name) {
                            server.mcp.invalidate();
                        }
                    }
                    Err(error) => {
                        current.insert(spec.name, error);
                    }
                    Ok(false) => {}
                }
            }
        }
        for (name, error) in &current {
            if failures.get(name) != Some(error) {
                eprintln!("(jeeves) MCP {name}: {error}");
                server.capabilities.invalidate();
            }
        }
        failures = current;
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}
