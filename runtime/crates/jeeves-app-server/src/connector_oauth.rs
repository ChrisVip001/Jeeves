//! Human-facing authorization, with the existing real loopback OAuth callback.
use jeeves_protocol::ConnectorOAuthStartResponse;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

enum Target {
    Feishu(jeeves_connector::config::FeishuConfig),
    Mcp {
        home: PathBuf,
        name: String,
        url: String,
    },
}

fn https_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}

fn target(home: &std::path::Path, id: &str) -> Result<Target, String> {
    if id == "connector:feishu" {
        let f = jeeves_connector::config::load(home)
            .map_err(|_| "invalid connector configuration")?
            .feishu
            .ok_or("Feishu is not configured")?;
        if f.identity() != Ok("user") || !https_url(&f.base_url) {
            return Err("configure Feishu as a user with an HTTPS service URL first".into());
        }
        return Ok(Target::Feishu(f));
    }
    let name = id
        .strip_prefix("mcp:")
        .filter(|s| !s.is_empty())
        .ok_or("this connector does not support OAuth initiation")?;
    for path in crate::server::mcp_live::config_paths(home) {
        if !path.exists() {
            continue;
        }
        let config =
            jeeves_mcp::config::Config::read(&path).map_err(|_| "invalid MCP configuration")?;
        if let Some(spec) = config.servers.into_iter().find(|s| s.name == name) {
            let url = spec
                .url
                .filter(|u| https_url(u))
                .ok_or("MCP OAuth requires an HTTPS service URL")?;
            if !spec.oauth.unwrap_or(false) {
                return Err("OAuth is not enabled for this MCP server".into());
            }
            return Ok(Target::Mcp {
                home: path.parent().unwrap_or(home).to_path_buf(),
                name: name.to_string(),
                url,
            });
        }
    }
    Err("no configured OAuth connector with that id".into())
}

impl crate::Server {
    pub(crate) async fn start_connector_oauth(
        &self,
        id: &str,
    ) -> Result<ConnectorOAuthStartResponse, String> {
        let home = self.settings().home;
        let target = target(&home, id)?;
        let lock = self.session_write_lock("connector-oauth");
        let guard = lock.try_lock_owned().map_err(
            |_| "an authorization is already in progress; complete or wait for it to expire",
        )?;
        let (tx, rx) = oneshot::channel::<Result<String, String>>();
        let pending = Arc::new(Mutex::new(Some(tx)));
        let callback = pending.clone();
        let cards = self.capabilities.clone();
        let mcp = self.mcp.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            let refresh_mcp = matches!(&target, Target::Mcp { .. });
            let ready = |url: &str| {
                if let Some(tx) = callback.lock().expect("OAuth ready slot lock").take() {
                    let _ = tx.send(if https_url(url) {
                        Ok(url.to_string())
                    } else {
                        Err("provider did not return a safe HTTPS authorization URL".into())
                    });
                }
            };
            let result = match target {
                Target::Feishu(f) => jeeves_connector::feishu_oauth::login_with(&f, &home, &ready)
                    .await
                    .is_ok(),
                Target::Mcp { home, name, url } => match reqwest::Client::builder()
                    .timeout(Duration::from_secs(15))
                    .build()
                {
                    Ok(client) => jeeves_mcp::oauth::sign_in(&client, &home, &name, &url, &ready)
                        .await
                        .is_ok(),
                    Err(_) => false,
                },
            };
            if let Some(tx) = pending.lock().expect("OAuth ready slot lock").take() {
                let _ = tx.send(Err(
                    "could not prepare authorization; check configuration and callback port".into(),
                ));
            }
            if result {
                if refresh_mcp {
                    mcp.invalidate();
                    if let Some((added, removed)) = mcp.refresh(&home).await {
                        eprintln!(
                            "(jeeves) OAuth connection refreshed: added [{}], removed [{}]",
                            added.join(", "),
                            removed.join(", ")
                        );
                    }
                }
                cards.invalidate();
                eprintln!(
                    "(jeeves) connector authorization completed; credentials stored, no values returned"
                );
            } else {
                eprintln!(
                    "(jeeves) connector authorization failed or expired; review the service and try again"
                );
            }
        });
        match tokio::time::timeout(Duration::from_secs(20), rx).await {
            Ok(Ok(Ok(authorization_url))) => Ok(ConnectorOAuthStartResponse { authorization_url }),
            Ok(Ok(Err(error))) => {
                task.abort();
                Err(error)
            }
            _ => {
                task.abort();
                Err("authorization preparation timed out".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_configured_oauth_targets_are_supported() {
        let home = tempfile::tempdir().unwrap();
        assert!(target(home.path(), "connector:email").is_err());
        assert!(target(home.path(), "mcp:missing").is_err());
        std::fs::write(home.path().join("mcp.json"),r#"{"servers":[{"name":"x","url":"https://example.test/mcp","oauth":true},{"name":"y","url":"https://example.test/mcp"}]}"#).unwrap();
        assert!(matches!(
            target(home.path(), "mcp:x"),
            Ok(Target::Mcp { .. })
        ));
        assert!(target(home.path(), "mcp:y").is_err());
        assert!(!https_url("https://name:secret@example.test/"));
    }
}
