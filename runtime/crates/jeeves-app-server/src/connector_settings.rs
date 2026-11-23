//! Human-owned configuration for native connectors.
pub(crate) mod publish;

use jeeves_connector::config::{self, ConnectorConfig, MailSecurity, MailServer};
use jeeves_protocol::{
    CalendarConnectorSettings, ConnectorSettingsResponse, ConnectorSettingsSaveParams,
    EmailConnectorSettings, MailConnectorSecurity, MailConnectorServer,
};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

pub(crate) fn register_native(
    registry: &mut jeeves_tools::ToolRegistry,
    id: &str,
    tool: jeeves_tools::Tool,
) {
    let effect = jeeves_mcp::manifest::builtin(id).tools[&tool.name].effect;
    if effect.is_read() {
        registry.register_with_effect(tool, effect);
    } else {
        registry.register_exclusive_with_effect(tool, effect);
    }
}

/// Rebuild the built-in connector portion of a tool snapshot from the complete validated
/// configuration. Old snapshots are untouched; invalid config disables all of these tools.
pub(crate) fn apply(home: &Path, base: &jeeves_tools::ToolRegistry) -> jeeves_tools::ToolRegistry {
    let mut tools = base.clone();
    for name in ["calendar_list", "email_read", "email_send", "feishu_send"] {
        tools = tools.without(name);
    }
    match config::load(home) {
        Ok(config) => {
            if let Some(c) = config.calendar {
                register_native(
                    &mut tools,
                    "calendar",
                    jeeves_connector::calendar::calendar_list_tool(c),
                );
            }
            if let Some(e) = config.email {
                register_native(
                    &mut tools,
                    "email",
                    jeeves_connector::email::email_read_tool(e.clone()),
                );
                if !e.allow_recipients.is_empty() {
                    register_native(
                        &mut tools,
                        "email",
                        jeeves_connector::email::email_send_tool(e),
                    );
                }
            }
            if let Some(f) = config.feishu.filter(|f| !f.allow_recipients.is_empty()) {
                register_native(&mut tools, "feishu", jeeves_connector::feishu_send_tool(f));
            }
        }
        Err(_) => eprintln!(
            "(jeeves) invalid connector configuration; connector tools disabled for this turn"
        ),
    }
    tools
}

fn pointer(source: &str) -> bool {
    if let Some(name) = source.strip_prefix("vault:") {
        jeeves_config::vault::valid_name(name)
    } else if let Some(name) = source.strip_prefix("env:") {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    } else {
        false
    }
}

fn safe_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

fn server_view(server: MailServer) -> MailConnectorServer {
    MailConnectorServer {
        host: server.host,
        port: i32::from(server.port),
        security: match server.security {
            MailSecurity::Tls => MailConnectorSecurity::Tls,
            MailSecurity::Starttls => MailConnectorSecurity::Starttls,
            MailSecurity::Plain => MailConnectorSecurity::Plain,
        },
    }
}

fn view(config: ConnectorConfig) -> ConnectorSettingsResponse {
    ConnectorSettingsResponse {
        calendar: config.calendar.map(|c| CalendarConnectorSettings {
            url: if safe_url(&c.url) {
                c.url
            } else {
                String::new()
            },
            username: c.username,
            credential_source: c.password.filter(|p| pointer(p)),
            timezone: c.timezone,
        }),
        email: config.email.map(|e| EmailConnectorSettings {
            username: e.username,
            credential_source: if pointer(&e.password) {
                e.password
            } else {
                String::new()
            },
            from: e.from,
            imap: server_view(e.imap),
            smtp: server_view(e.smtp),
            allow_recipients: e.allow_recipients,
            live: e.live,
        }),
    }
}

fn mail_server(server: MailConnectorServer) -> Result<Value, String> {
    if server.host.is_empty()
        || server.host.len() > 253
        || !server
            .host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || !(1..=65535).contains(&server.port)
    {
        return Err("enter a hostname and a port between 1 and 65535".into());
    }
    if matches!(server.security, MailConnectorSecurity::Plain) {
        return Err("plaintext mail connections cannot be configured through Settings".into());
    }
    Ok(json!({"host":server.host,"port":server.port,"security":server.security}))
}

fn save(
    home: &Path,
    params: ConnectorSettingsSaveParams,
) -> Result<ConnectorSettingsResponse, String> {
    let path = home.join("connectors.json");
    let mut raw = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str::<Value>(&raw)
            .map_err(|_| "connectors.json is invalid; repair it before saving".to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(format!("could not read connector configuration: {error}")),
    };
    let old: ConnectorConfig = serde_json::from_value(raw.clone())
        .map_err(|_| "connectors.json is invalid; repair it before saving".to_string())?;
    config::validate(&old)
        .map_err(|_| "connectors.json is invalid; repair it before saving".to_string())?;
    match (params.calendar, params.email) {
        (Some(c), None) => {
            if !safe_url(&c.url) {
                return Err(
                    "calendar URL must be HTTPS, without embedded credentials, query or fragment"
                        .into(),
                );
            }
            if c.username.is_some() != c.credential_source.is_some()
                || c.username.as_ref().is_some_and(|s| s.trim().is_empty())
                || c.credential_source.as_ref().is_some_and(|s| !pointer(s))
            {
                return Err("calendar username and env:NAME or vault:NAME credential reference must be supplied together".into());
            }
            raw["calendar"] = json!({"url":c.url,"username":c.username,"password":c.credential_source,"timezone":c.timezone});
        }
        (None, Some(e)) => {
            if e.username.trim().is_empty()
                || !pointer(&e.credential_source)
                || e.allow_recipients
                    .iter()
                    .any(|s| s.trim().is_empty() || s.contains(['\r', '\n']))
                || (e.live && e.allow_recipients.is_empty())
            {
                return Err("email needs a username, a credential reference and, for live send, a recipient allowlist".into());
            }
            let imap = mail_server(e.imap)?;
            let smtp = mail_server(e.smtp)?;
            raw["email"] = json!({"username":e.username,"password":e.credential_source,"from":e.from,"imap":imap,"smtp":smtp,"allow_recipients":e.allow_recipients,"live":e.live});
        }
        _ => return Err("save exactly one connector section".into()),
    }
    let config: ConnectorConfig = serde_json::from_value(raw.clone())
        .map_err(|_| "invalid connector settings".to_string())?;
    config::validate(&config).map_err(|_| {
        "invalid connector settings (check mailbox address or timezone)".to_string()
    })?;
    let mut tmp = tempfile::NamedTempFile::new_in(home).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&raw).map_err(|e| e.to_string())?;
    tmp.write_all(&bytes)
        .and_then(|_| tmp.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(view(config))
}

impl crate::Server {
    pub(crate) fn connector_settings(&self) -> Result<ConnectorSettingsResponse, String> {
        config::load(&self.settings().home)
            .map(view)
            .map_err(|_| "connectors.json is invalid or unreadable".into())
    }
    pub(crate) async fn save_connector_settings(
        &self,
        params: ConnectorSettingsSaveParams,
    ) -> Result<ConnectorSettingsResponse, String> {
        let lock = self.session_write_lock("connector-settings");
        let _guard = lock.lock().await;
        let result = save(&self.settings().home, params)?;
        self.capabilities.invalidate();
        eprintln!("(jeeves) connector settings saved; effective in the next turn");
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_save_is_atomic_and_does_not_echo_legacy_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("connectors.json");
        let initial = json!({"feishu":{"app_id":"fixture","app_secret":"env:APP_SECRET"},"calendar":{"url":"https://user:SECRET@example.test/cal","username":"fixture","password":"SECRET","timezone":"UTC"}});
        std::fs::write(&path, initial.to_string()).unwrap();
        let v = view(config::load(dir.path()).unwrap());
        assert!(!serde_json::to_string(&v).unwrap().contains("SECRET"));
        let params = |source: &str| ConnectorSettingsSaveParams {
            calendar: Some(CalendarConnectorSettings {
                url: "https://example.test/cal".into(),
                username: Some("fixture".into()),
                credential_source: Some(source.into()),
                timezone: "UTC".into(),
            }),
            email: None,
        };
        assert!(save(dir.path(), params("SECRET")).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), initial.to_string());
        let saved = save(dir.path(), params("vault:CALENDAR")).unwrap();
        assert_eq!(
            saved.calendar.unwrap().credential_source.as_deref(),
            Some("vault:CALENDAR")
        );
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(raw["feishu"], initial["feishu"]);
    }

    #[test]
    fn new_turn_observes_connector_changes_and_held_table_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let base = jeeves_tools::ToolRegistry::new();
        let held = apply(dir.path(), &base);
        save(
            dir.path(),
            ConnectorSettingsSaveParams {
                calendar: Some(CalendarConnectorSettings {
                    url: "https://example.test/cal".into(),
                    username: None,
                    credential_source: None,
                    timezone: "UTC".into(),
                }),
                email: None,
            },
        )
        .unwrap();
        let next = apply(dir.path(), &base);
        assert!(next.names().contains(&"calendar_list"));
        assert!(held.names().is_empty());
        std::fs::write(dir.path().join("connectors.json"), "broken").unwrap();
        assert!(apply(dir.path(), &next).names().is_empty());
    }

    #[test]
    fn settings_accept_valid_values_without_undocumented_field_limits() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("https://calendar.example.test/{}", "a".repeat(2048));
        let saved = save(
            dir.path(),
            ConnectorSettingsSaveParams {
                calendar: Some(CalendarConnectorSettings {
                    url: url.clone(),
                    username: None,
                    credential_source: None,
                    timezone: "UTC".into(),
                }),
                email: None,
            },
        )
        .unwrap();
        assert_eq!(saved.calendar.unwrap().url, url);

        let mail = MailConnectorServer {
            host: "mail.example.test".into(),
            port: 993,
            security: MailConnectorSecurity::Tls,
        };
        let recipients = Vec::from_fn(101, |n| format!("user{n}@example.test"));
        let username = "a".repeat(321);
        let source = format!("env:{}", "A".repeat(101));
        let saved = save(
            dir.path(),
            ConnectorSettingsSaveParams {
                calendar: None,
                email: Some(EmailConnectorSettings {
                    username: username.clone(),
                    credential_source: source.clone(),
                    from: "Fixture <reader@example.test>".into(),
                    imap: mail.clone(),
                    smtp: MailConnectorServer { port: 465, ..mail },
                    allow_recipients: recipients.clone(),
                    live: true,
                }),
            },
        )
        .unwrap();
        let email = saved.email.unwrap();
        assert_eq!(email.allow_recipients, recipients);
        assert_eq!(email.username, username);
        assert_eq!(email.credential_source, source);
    }
}
