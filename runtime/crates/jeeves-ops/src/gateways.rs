//! Chat gateways hosted by the Runtime: the owner connects Lark in Settings and the Runtime
//! starts, stops and watches the gateway process — no second terminal, no `.env`.
//!
//! Non-secret settings live in `<home>/gateways.json`; the app secret goes to the secret store
//! (keychain first). The process only ever receives the secret through its environment.
//! "Never silent": every start, exit and refusal is a stderr line and part of the status.

use anyhow::{bail, Context, Result};
use jeeves_config::models::SecretStore;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

const SECRET_ACCOUNT: &str = "gateway:lark:app-secret";
const LOG_LINES: usize = 20;
const CLAIM_MARK: &str = "claim candidate: ";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LarkConfig {
    pub enabled: bool,
    pub app_id: String,
    /// open_ids (`ou_…`) allowed to talk to Jeeves. Empty = nobody until an owner is confirmed.
    pub allow: Vec<String>,
    pub dispatchers: Vec<String>,
    /// `lark` for the international domain; anything else is Feishu.
    pub domain: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LarkStatus {
    pub config: LarkConfig,
    pub has_secret: bool,
    /// `stopped` / `running` / `exited`
    pub state: &'static str,
    /// The first sender while no owner is set, waiting for the owner's confirmation.
    pub candidate: Option<String>,
    pub recent: Vec<String>,
}

#[derive(Default)]
struct Live {
    child: Option<Child>,
    exited: bool,
    candidate: Option<String>,
    recent: VecDeque<String>,
}

pub struct Gateways {
    home: PathBuf,
    token: Option<String>,
    url: Mutex<Option<String>>,
    live: Arc<Mutex<Live>>,
    secrets: Mutex<SecretStore>,
}

fn path(home: &Path) -> PathBuf {
    home.join("gateways.json")
}

pub fn load(home: &Path) -> LarkConfig {
    std::fs::read_to_string(path(home))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(home: &Path, config: &LarkConfig) -> Result<()> {
    std::fs::create_dir_all(home).ok();
    std::fs::write(path(home), serde_json::to_string_pretty(config)?)
        .context("could not write gateways.json")
}

/// `integrations/channels/src/bin-lark.ts` next to this checkout (override: `JEEVES_GATEWAY_BIN`).
fn gateway_bin() -> PathBuf {
    match std::env::var_os("JEEVES_GATEWAY_BIN") {
        Some(bin) => PathBuf::from(bin),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../integrations/channels/src/bin-lark.ts"),
    }
}

fn clean(list: &[String]) -> Vec<String> {
    list.iter()
        .map(|entry| entry.trim().to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

impl Gateways {
    pub fn new(home: &Path, token: Option<String>) -> Arc<Self> {
        Self::with_secrets(home, token, SecretStore::auto())
    }

    pub fn with_secrets(home: &Path, token: Option<String>, secrets: SecretStore) -> Arc<Self> {
        Arc::new(Self {
            home: home.to_path_buf(),
            token,
            url: Mutex::new(None),
            live: Arc::new(Mutex::new(Live::default())),
            secrets: Mutex::new(secrets),
        })
    }

    fn secret(&self) -> Option<String> {
        self.secrets
            .lock()
            .expect("gateway secrets lock")
            .get(&self.home, SECRET_ACCOUNT)
    }

    /// The address the gateway attaches to: known only once the listener is bound.
    pub fn set_url(&self, url: String) {
        *self.url.lock().expect("gateway url lock") = Some(url);
    }

    pub fn status(&self) -> LarkStatus {
        let config = load(&self.home);
        let mut live = self.live.lock().expect("gateway live lock");
        let running = match live.child.as_mut() {
            Some(child) => child.try_wait().ok().flatten().is_none(),
            None => false,
        };
        let state = if running {
            "running"
        } else if live.exited {
            "exited"
        } else {
            "stopped"
        };
        LarkStatus {
            has_secret: self.secret().is_some(),
            config,
            state,
            candidate: live.candidate.clone(),
            recent: live.recent.iter().cloned().collect(),
        }
    }

    /// Save the settings. A new secret goes to the secret store; `None` keeps the old one.
    /// Returns where the secret landed, when one was given.
    pub fn save(
        &self,
        mut config: LarkConfig,
        secret: Option<&str>,
    ) -> Result<Option<&'static str>> {
        config.app_id = config.app_id.trim().to_string();
        config.allow = clean(&config.allow);
        config.dispatchers = clean(&config.dispatchers);
        if let Some(bad) = config
            .allow
            .iter()
            .chain(&config.dispatchers)
            .find(|id| !id.starts_with("ou_"))
        {
            bail!("{bad} is not an open_id: Lark ids start with ou_ (an App ID or a phone number matches nobody)");
        }
        let landed = match secret.filter(|s| !s.is_empty()) {
            Some(secret) => Some(
                self.secrets
                    .lock()
                    .expect("gateway secrets lock")
                    .put(&self.home, SECRET_ACCOUNT, secret)
                    .map_err(anyhow::Error::msg)?,
            ),
            None => None,
        };
        save(&self.home, &config)?;
        Ok(landed)
    }

    /// Start (or restart) the gateway from the saved settings.
    pub async fn start(&self) -> Result<()> {
        self.stop().await;
        let config = load(&self.home);
        if config.app_id.is_empty() {
            bail!("enter the App ID first");
        }
        let secret = self.secret().context("enter the App Secret first")?;
        let url = self
            .url
            .lock()
            .expect("gateway url lock")
            .clone()
            .context("the Runtime is still starting")?;
        let mut command = Command::new("node");
        command
            .arg(gateway_bin())
            .env("LARK_APP_ID", &config.app_id)
            .env("LARK_APP_SECRET", secret)
            .env("LARK_ALLOW", config.allow.join(","))
            .env("LARK_DISPATCHERS", config.dispatchers.join(","))
            .env("LARK_DOMAIN", &config.domain)
            .env(
                "LARK_CLAIM",
                if config.allow.is_empty() { "1" } else { "0" },
            )
            .env("JEEVES_GATEWAY_URL", url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        if let Some(token) = &self.token {
            command.env("JEEVES_OPS_TOKEN", token);
        }
        let mut child = command
            .spawn()
            .context("could not start the gateway (is Node.js installed?)")?;
        let stderr = child.stderr.take().context("gateway stderr")?;
        {
            let mut live = self.live.lock().expect("gateway live lock");
            *live = Live {
                child: Some(child),
                ..Live::default()
            };
        }
        let live = Arc::clone(&self.live);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut live = live.lock().expect("gateway live lock");
                if let Some(id) = line.split(CLAIM_MARK).nth(1) {
                    live.candidate = Some(id.trim().to_string());
                }
                if live.recent.len() == LOG_LINES {
                    live.recent.pop_front();
                }
                live.recent.push_back(line);
            }
            let mut live = live.lock().expect("gateway live lock");
            if live.child.is_some() {
                live.exited = true;
                eprintln!(
                    "(jeeves) the Lark gateway exited; see Settings → Connections for the last lines"
                );
            }
        });
        eprintln!("(jeeves) Lark gateway started (app {})", config.app_id);
        Ok(())
    }

    pub async fn stop(&self) {
        let child = {
            let mut live = self.live.lock().expect("gateway live lock");
            live.exited = false;
            live.child.take()
        };
        if let Some(mut child) = child {
            let _ = child.kill().await;
            eprintln!("(jeeves) Lark gateway stopped");
        }
    }

    /// The owner confirms the candidate: it becomes the allowlist and the gateway restarts.
    pub async fn claim(&self, open_id: &str) -> Result<()> {
        let mut config = load(&self.home);
        if !config.allow.is_empty() {
            bail!("an owner is already set");
        }
        let candidate = self
            .live
            .lock()
            .expect("gateway live lock")
            .candidate
            .clone();
        if candidate.as_deref() != Some(open_id) {
            bail!("{open_id} is not the sender waiting for confirmation");
        }
        config.allow = vec![open_id.to_string()];
        save(&self.home, &config)?;
        self.start().await
    }

    /// Start at boot when the owner left it on and it is fully configured.
    pub async fn autostart(&self) {
        let config = load(&self.home);
        if config.enabled && !config.app_id.is_empty() {
            if let Err(why) = self.start().await {
                eprintln!("(jeeves) the Lark gateway did not start: {why:#}");
            }
        }
    }
}

#[cfg(test)]
#[path = "gateways_tests.rs"]
mod tests;
