//! `<home>/connectors.json` — intent, not secrets. The file says *which* connector and
//! where its password comes from; the password itself lives in the `SecretStore`
//! (keychain first, plaintext fallback, and the caller is told which).
//!
//! ```json
//! {
//!   "feishu": {
//!     "app_id": "cli_a…",
//!     "app_secret": "keychain:connector:feishu",
//!     "allow_recipients": ["oc_chat_id", "ou_user_id"],
//!     "live": false
//!   }
//! }
//! ```
//!
//! `app_secret` takes `env:VAR` or anything else → `SecretStore` lookup under that account
//! name. `live: false` (the default) means drafts only; turning it on is a human act.

use std::path::Path;

use jeeves_config::models::SecretStore;
use serde::Deserialize;

/// The whole file. Unknown keys are rejected: a typo'd `allow_recipents` silently
/// meaning "allow nobody" is exactly the kind of quiet failure this repo refuses.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectorConfig {
    #[serde(default)]
    pub feishu: Option<FeishuConfig>,
    #[serde(default)]
    pub calendar: Option<CalendarConfig>,
    #[serde(default)]
    pub email: Option<EmailConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalendarConfig {
    /// An HTTPS CalDAV collection URL, or a direct .ics feed.
    pub url: String,
    #[serde(default)]
    pub username: Option<String>,
    /// `env:VAR` or a SecretStore account name; never the password itself.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailConfig {
    pub username: String,
    /// `env:VAR` or a SecretStore account name; never the password itself.
    pub password: String,
    pub from: String,
    pub imap: MailServer,
    pub smtp: MailServer,
    #[serde(default)]
    pub allow_recipients: Vec<String>,
    #[serde(default)]
    pub live: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MailServer {
    pub host: String,
    pub port: u16,
    pub security: MailSecurity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MailSecurity {
    Tls,
    Starttls,
    Plain,
}

fn default_timezone() -> String {
    "UTC".into()
}

pub(crate) fn resolve_secret(home: &Path, source: &str) -> Option<String> {
    match source.strip_prefix("env:") {
        Some(var) => std::env::var(var).ok().filter(|v| !v.trim().is_empty()),
        None => SecretStore::auto().get(home, source),
    }
}

impl CalendarConfig {
    pub fn secret(&self, home: &Path) -> Option<String> {
        self.password
            .as_deref()
            .and_then(|source| resolve_secret(home, source))
    }
}

impl EmailConfig {
    pub fn secret(&self, home: &Path) -> Option<String> {
        resolve_secret(home, &self.password)
    }

    pub fn permits(&self, recipient: &str) -> bool {
        let target = recipient.trim().to_ascii_lowercase();
        self.allow_recipients
            .iter()
            .any(|entry| entry.trim().to_ascii_lowercase() == target)
    }
}

/// A self-built Feishu (Lark) app. Messages go out as **the app**
/// (tenant_access_token) — that is the part a bot can do; acting as a user needs the OAuth
/// code flow plus a callback registered in the console. See docs/limitations: no
/// unverifiable flow is written before there is an app to verify it against.
///
/// ```json
/// {
///   "feishu": {
///     "app_id": "cli_a…",
///     "app_secret": "keychain:connector:feishu",
///     "allow_recipients": ["oc_chat_id", "ou_user_id"],
///     "live": false
///   }
/// }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeishuConfig {
    pub app_id: String,
    /// `env:VAR` or a `SecretStore` account name.
    pub app_secret: String,
    /// Who the message comes from: `app` (default, tenant token) or `user` (the person who
    /// ran `jeeves connector feishu login`). The human's decision, never guessed.
    #[serde(default, rename = "as")]
    pub identity: Option<String>,
    /// `open_id` (a person) or `chat_id` (a group) — must match the ids in the allowlist.
    #[serde(default = "default_receive_id_type")]
    pub receive_id_type: String,
    /// Hard boundary, empty = nobody.
    #[serde(default)]
    pub allow_recipients: Vec<String>,
    /// `false` (default): drafts to `<home>/outbox/`.
    #[serde(default)]
    pub live: bool,
    /// Override for tests and self-hosted Lark deployments; defaults to the public cloud.
    #[serde(default = "default_feishu_base")]
    pub base_url: String,
}

fn default_receive_id_type() -> String {
    "open_id".to_string()
}

fn default_feishu_base() -> String {
    "https://open.feishu.cn".to_string()
}

impl FeishuConfig {
    /// `app` (default) or `user` — validated at load so a typo cannot silently send as
    /// the wrong identity.
    pub fn identity(&self) -> Result<&str, String> {
        match self.identity.as_deref().unwrap_or("app") {
            identity @ ("app" | "user") => Ok(identity),
            other => Err(format!(
                "feishu.as must be app or user, got {other:?} (empty = app)"
            )),
        }
    }

    pub fn permits(&self, receive_id: &str) -> bool {
        let target = receive_id.trim().to_ascii_lowercase();
        !target.is_empty()
            && self
                .allow_recipients
                .iter()
                .any(|entry| entry.trim().to_ascii_lowercase() == target)
    }

    /// Resolve the app secret (keychain / env).
    pub fn secret(&self, home: &Path) -> Option<String> {
        resolve_secret(home, &self.app_secret)
    }
}

/// Load the file. Missing file = no connectors (not an error); a malformed file is.
pub fn load(home: &Path) -> anyhow::Result<ConnectorConfig> {
    let config = load_raw(home)?;
    validate(&config)?;
    Ok(config)
}

pub fn validate(config: &ConnectorConfig) -> anyhow::Result<()> {
    if let Some(feishu) = &config.feishu {
        feishu
            .identity()
            .map_err(|e| anyhow::anyhow!("feishu in connectors.json has a problem: {e}"))?;
    }
    if let Some(calendar) = &config.calendar {
        if calendar.username.is_some() != calendar.password.is_some() {
            anyhow::bail!("calendar username and password must be configured together");
        }
        calendar
            .timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| anyhow::anyhow!("calendar.timezone must be an IANA timezone"))?;
    }
    if let Some(email) = &config.email {
        email
            .from
            .parse::<lettre::message::Mailbox>()
            .map_err(|e| anyhow::anyhow!("email.from is not a valid mailbox: {e}"))?;
        if email.imap.host.trim().is_empty() || email.smtp.host.trim().is_empty() {
            anyhow::bail!("email IMAP and SMTP hosts must not be empty");
        }
    }
    Ok(())
}

fn load_raw(home: &Path) -> anyhow::Result<ConnectorConfig> {
    let path = home.join("connectors.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ConnectorConfig::default());
        }
        Err(error) => return Err(error.into()),
    };
    serde_json::from_str(&raw).map_err(|e| {
        anyhow::anyhow!(
            "{} could not be read: {e}\n\n             connector config is hand-written; a mistake leaves nobody able to use it — better to fail loudly than to guess.",
            path.display()
        )
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
