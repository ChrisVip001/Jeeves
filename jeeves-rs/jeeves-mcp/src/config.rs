//! How `mcp.json` is read, and judging "what this config actually means".
//!
//! Two kinds of things in the config are rejected on the spot, same reason: **writing both
//! spellings means saying nothing**. Which wins is a guess; guessing wrong shows up as
//! "connected but on the other transport" — the hardest fault to trace, so we reject.
//!
//! ```json
//! {"servers": [
//!   {"name": "fs", "command": "npx",
//!    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]},
//!   {"name": "notes", "url": "https://host/mcp", "auth_env": "NOTES_API_KEY"}
//! ]}
//! ```
//!
//! `auth_env` holds the **name of an environment variable**, not the credential itself —
//! mcp.json gets pasted into bug reports; a bearer token in it is a leak. The variable's
//! value is sent as `Authorization: Bearer <value>`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One server entry. All fields optional — "required" is judged by shape in [`resolve`],
/// so the error reads "you wrote both", not serde's "missing field".
///
/// `Serialize` is here for one reason: the dashboard's capability center rewrites
/// mcp.json (add / remove / replace a server), and a config type that can only be read
/// would force a second, hand-rolled writer. `skip_serializing_if` keeps the file as
/// clean as a hand-written one — no `"url": null` noise.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ServerSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<crate::manifest::ManifestSource>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_switches: BTreeMap<String, bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(alias = "authEnv")]
    pub auth_env: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub servers: Vec<ServerSpec>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        serde_json::from_str(text).map_err(|e| format!("mcp.json is not valid JSON: {e}"))
    }

    pub fn read(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        Config::parse(&text)
    }
}

/// Serialize the dashboard and Runtime read-modify-write operations on this home.
pub fn edit(
    home: &Path,
    update: impl FnOnce(&mut Config) -> Result<(), String>,
) -> Result<(), String> {
    use std::io::{ErrorKind, Write};
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join("mcp-config.lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    let path = home.join("mcp.json");
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text)?,
        Err(e) if e.kind() == ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e.to_string()),
    };
    update(&mut cfg)?;
    let mut tmp = tempfile::NamedTempFile::new_in(home).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(tmp.as_file_mut(), &cfg).map_err(|e| e.to_string())?;
    tmp.flush().map_err(|e| e.to_string())?;
    tmp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

impl ServerSpec {
    pub fn effective_manifest(
        &self,
        root: &Path,
    ) -> Result<Option<jeeves_protocol::ConnectorManifest>, String> {
        use jeeves_protocol::{ConnectorCategory, ConnectorManifest, ConnectorToolRule};
        let mut manifest = self.manifest.as_ref().map(|s| s.load(root)).transpose()?;
        for (name, enabled) in &self.tool_switches {
            let m = manifest.get_or_insert_with(|| ConnectorManifest {
                category: ConnectorCategory::Other,
                verbs: Default::default(),
                tools: Default::default(),
            });
            m.tools
                .entry(name.clone())
                .or_insert_with(|| ConnectorToolRule {
                    effect: Default::default(),
                    resource: None,
                    disabled: false,
                })
                .disabled = !enabled;
        }
        if !self.tool_switches.is_empty() {
            if let Some(m) = &manifest {
                crate::manifest::validate(m)?;
            }
        }
        Ok(manifest)
    }
}

/// Which config files to read: `<home>/mcp.json` (the user's own) plus the
/// colon-separated `extra` paths (the operator's — machine-wide or bundled packs;
/// `JEEVES_MCP_CONFIG` in production). Same split and same reason as
/// `SkillLoader::dirs_for`: **the user's file is the only one the dashboard lets a
/// human edit**; the extras show up greyed out, "built-in", because deleting them
/// from one machine's UI would silently un-configure every machine that shares them.
///
/// `extra` is a parameter, not an env read inside, so this stays pure-function
/// testable (same discipline as [`resolve`]).
pub fn config_paths_for(home: &Path, extra: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let mut paths = vec![home.join("mcp.json")];
    if let Some(extra) = extra {
        paths.extend(std::env::split_paths(extra));
    }
    paths
}

/// The connection decoded from one entry. The transport is decided by the config's **shape**,
/// not a `transport` field — either a local command or a remote url.
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    OAuth {
        url: String,
        store: crate::oauth::TokenStore,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        /// `Some` means **only these variables** (env set = replace, not append);
        /// `None` means inherit the current process environment.
        env: Option<BTreeMap<String, String>>,
    },
    Http {
        url: String,
        /// The decoded bearer value (not the variable name).
        token: Option<String>,
    },
}

/// Decode one config entry into a connection, or explain what is wrong with it.
///
/// `lookup_env` is passed in instead of reading the process env, so this is pure-function testable.
pub fn resolve(
    spec: &ServerSpec,
    lookup_env: &dyn Fn(&str) -> Option<String>,
) -> Result<Transport, String> {
    let url = spec.url.as_deref().unwrap_or("");
    let command = spec.command.as_deref().unwrap_or("");

    if !url.is_empty() && !command.is_empty() {
        return Err("both 'url' and 'command' are set — pick one transport".to_string());
    }
    if url.is_empty() && command.is_empty() {
        return Err("neither 'url' nor 'command' is set — nothing to connect to".to_string());
    }

    if url.is_empty() {
        return Ok(Transport::Stdio {
            command: command.to_string(),
            args: spec.args.clone().unwrap_or_default(),
            env: spec.env.clone(),
        });
    }

    let auth_env = spec.auth_env.as_deref().unwrap_or("");
    let use_oauth = spec.oauth.unwrap_or(false);
    if !auth_env.is_empty() && use_oauth {
        return Err("both 'auth_env' and 'oauth' are set — pick one".to_string());
    }
    if !auth_env.is_empty() {
        let token = lookup_env(auth_env)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                format!("{auth_env} has no value (that is what 'auth_env' in mcp.json points at)")
            })?;
        return Ok(Transport::Http {
            url: url.to_string(),
            token: Some(token),
        });
    }
    Ok(Transport::Http {
        url: url.to_string(),
        token: None,
    })
}

/// When a remote server cannot be reached, tell the person the one thing **they can check themselves**.
///
/// This hint only makes sense for the `auth_env` path: oauth servers use tokens from
/// `mcp-auth/`, and their errors already explain themselves.
pub fn auth_hint(transport: &Transport) -> Option<String> {
    match transport {
        Transport::Http { url, token: None } => Some(format!(
            "  {url} — if it needs auth, check that the variable 'auth_env' points at holds valid credentials"
        )),
        _ => None,
    }
}

/// `<server>_<tool>`, squeezed into a shape model vendors accept.
///
/// Tool names hitting Anthropic / OpenAI must pass `^[a-zA-Z0-9_-]{1,64}$`; MCP itself has
/// no such limit, and dot separators are common (many servers send `memory.remember`-style
/// names), so "a perfectly legal MCP server" would get the request rejected before the
/// model sees anything.
///
/// Only the name the model reads changes; the original still goes back to the server, so
/// renaming cannot break dispatch. Same as [`model_safe_name`], but **unique** in `taken`.
///
/// Why needed: `{server}_{tool}` collapses — server `a`'s tool `b_c` and server `a_b`'s
/// tool `c` both become `a_b_c`. Two servers reporting a same-named tool happens; "who
/// overwrites whom" depends on registration order — the hardest bug class. Suffix `_2`, `_3`…
///
/// Returns `(final name, whether renamed)`: **a rename must be announced** — model and user
/// both see this name; a silent rename means debugging against a mismatched tool list.
pub fn model_safe_name_unique(
    server: &str,
    tool: &str,
    taken: &mut std::collections::HashSet<String>,
) -> (String, bool) {
    let base = model_safe_name(server, tool);
    if taken.insert(base.clone()) {
        return (base, false);
    }
    for n in 2u32.. {
        let candidate = with_suffix(&base, n);
        if taken.insert(candidate.clone()) {
            return (candidate, true);
        }
    }
    unreachable!("a free slot is always found")
}

/// Add a suffix under the 64-char cap: **keep the head** (the server prefix distinguishes best).
fn with_suffix(base: &str, n: u32) -> String {
    let suffix = format!("_{n}");
    let keep = 64usize.saturating_sub(suffix.len());
    let head: String = base.chars().take(keep).collect();
    format!("{head}{suffix}")
}

pub fn model_safe_name(server: &str, tool: &str) -> String {
    let safe: String = format!("{server}_{tool}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.len() <= 64 {
        return safe;
    }
    safe[safe.len() - 64..].trim_start_matches('_').to_string()
}
