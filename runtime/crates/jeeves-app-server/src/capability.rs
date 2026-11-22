//! Capability cards (`capability/list`, `capability/probe`): one read-only card per model
//! profile, MCP server and connector, saying whether it can be used now and, if not, why.
//!
//! A probe never returns a secret, only `secretPointer` (`env:NAME` / `keychain` / `file`).
//! Probing is on demand and cached in memory for 60 seconds per card.

use jeeves_config::models::{ModelsFile, SecretVault};
use jeeves_protocol::{CapabilityCard, CapabilityKind, CapabilityState};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// A card is re-probed at most this often.
pub(crate) const CACHE_SECS: i64 = 60;

/// Everything a probe may look at. Injected so tests never touch the real environment or keychain.
pub(crate) struct ProbeEnv<'a> {
    pub home: &'a Path,
    pub now: i64,
    pub env: &'a dyn Fn(&str) -> Option<String>,
    pub vault: &'a mut dyn SecretVault,
    pub models: &'a ModelsFile,
    /// MCP servers that connected at boot.
    pub connected_mcp: &'a [String],
    pub missing_scope_mcp: &'a [String],
    /// Extra MCP config files (`JEEVES_MCP_CONFIG`).
    pub mcp_extra: Option<&'a std::ffi::OsStr>,
}

/// One source of capability cards. Implementations live here, not in the tool layer.
pub(crate) trait CapabilityProbe: Send + Sync {
    fn cards(&self, env: &mut ProbeEnv<'_>) -> Vec<CapabilityCard>;
}

fn card(
    id: String,
    kind: CapabilityKind,
    title: String,
    tool_source: String,
    now: i64,
) -> CapabilityCard {
    CapabilityCard {
        manifest: None,
        id,
        kind,
        title,
        identity: None,
        tool_source,
        state: CapabilityState::Ready,
        detail: None,
        secret_pointer: None,
        last_checked_at: i32::try_from(now).ok(),
    }
}

/// Where a credential named by a `key_source`-style string lives. Never the value.
fn pointer_of(e: &mut ProbeEnv<'_>, source: &str, account: &str) -> (String, bool) {
    if let Some(var) = source.strip_prefix("env:") {
        (
            source.to_string(),
            (e.env)(var).is_some_and(|v| !v.trim().is_empty()),
        )
    } else if let Some(location) = e.vault.location(e.home, account) {
        (location.into(), true)
    } else {
        ("vault".into(), false)
    }
}

pub(crate) struct ModelProbe;

impl CapabilityProbe for ModelProbe {
    fn cards(&self, e: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let mut out = Vec::new();
        for p in &e.models.profiles {
            let mut c = card(
                format!("model:{}", p.id),
                CapabilityKind::Model,
                p.id.clone(),
                "model".into(),
                e.now,
            );
            c.identity = Some(match &p.model {
                Some(m) => format!("{} / {m}", p.provider),
                None => p.provider.clone(),
            });
            if let Some(source) = p.key_source.as_deref() {
                let (pointer, present) =
                    if p.credential_slot == Default::default() || source.starts_with("env:") {
                        pointer_of(e, source, &p.id)
                    } else {
                        match e
                            .vault
                            .versioned_credential(e.home, &p.credential_slot.account(&p.id))
                        {
                            Some((_, location)) => (location.into(), true),
                            None => ("vault".into(), false),
                        }
                    };
                c.secret_pointer = Some(pointer.clone());
                if !present {
                    c.state = CapabilityState::Unauthorized;
                    c.detail = Some(format!("no key found at {pointer}"));
                }
            }
            out.push(c);
        }
        out
    }
}

pub(crate) struct McpProbe;

impl CapabilityProbe for McpProbe {
    fn cards(&self, e: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for path in jeeves_mcp::config::config_paths_for(e.home, e.mcp_extra) {
            if !path.exists() {
                continue;
            }
            let config = match jeeves_mcp::config::Config::read(&path) {
                Ok(c) => c,
                Err(error) => {
                    let mut c = card(
                        format!("mcp:{}", path.display()),
                        CapabilityKind::Mcp,
                        path.display().to_string(),
                        "mcp".into(),
                        e.now,
                    );
                    c.state = CapabilityState::Unknown;
                    c.detail = Some(error);
                    out.push(c);
                    continue;
                }
            };
            for spec in config.servers {
                if spec.name.is_empty() || !seen.insert(spec.name.clone()) {
                    continue;
                }
                out.push(mcp_card(&spec, path.parent().unwrap_or(e.home), e));
            }
        }
        out
    }
}

fn mcp_card(
    spec: &jeeves_mcp::config::ServerSpec,
    auth_home: &Path,
    e: &ProbeEnv<'_>,
) -> CapabilityCard {
    let mut c = card(
        format!("mcp:{}", spec.name),
        CapabilityKind::Mcp,
        spec.name.clone(),
        format!("mcp:{}", spec.name),
        e.now,
    );
    match spec.effective_manifest(auth_home) {
        Ok(manifest) => c.manifest = manifest,
        Err(error) => {
            c.state = CapabilityState::BlockedByPolicy;
            c.detail = Some(error);
            return c;
        }
    }
    if spec.oauth.unwrap_or(false) {
        c.secret_pointer = Some("file".into());
        let auth = jeeves_mcp::oauth::TokenStore::new(auth_home, &spec.name).load();
        if !auth.matches_resource(spec.url.as_deref().unwrap_or_default()) {
            c.state = CapabilityState::Unauthorized;
            c.detail = Some("the connection address changed; sign in again".into());
            return c;
        }
        match auth.tokens {
            None => {
                c.state = CapabilityState::Unauthorized;
                c.detail = Some(format!(
                    "not signed in: run `jeeves mcp login {}`",
                    spec.name
                ));
            }
            Some(t) if t.expired() => {
                c.state = CapabilityState::Expired;
                c.detail = Some("the sign-in has expired".into());
            }
            Some(_) => {}
        }
    } else if let Some(var) = spec.auth_env.as_deref().filter(|v| !v.is_empty()) {
        c.secret_pointer = Some(format!("env:{var}"));
        if (e.env)(var).is_none_or(|v| v.is_empty()) {
            c.state = CapabilityState::Unauthorized;
            c.detail = Some(format!("{var} has no value"));
        }
    }
    if c.state == CapabilityState::Ready && !e.connected_mcp.contains(&spec.name) {
        if e.missing_scope_mcp.contains(&spec.name) {
            c.state = CapabilityState::MissingScope;
            c.detail = Some("the server explicitly reported insufficient OAuth scope".into());
        } else {
            c.state = CapabilityState::Offline;
            c.detail = Some("did not connect at startup".into());
        }
    }
    c
}

pub(crate) struct FeishuProbe;

impl CapabilityProbe for FeishuProbe {
    fn cards(&self, e: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let mut c = card(
            "connector:feishu".into(),
            CapabilityKind::Connector,
            "Feishu".into(),
            "feishu_send".into(),
            e.now,
        );
        c.manifest = Some(jeeves_mcp::manifest::builtin("feishu").clone());
        let config = match jeeves_connector::config::load(e.home) {
            Ok(cfg) => cfg,
            Err(error) => {
                c.state = CapabilityState::Unknown;
                c.detail = Some(error.to_string());
                return vec![c];
            }
        };
        let Some(feishu) = config.feishu else {
            return Vec::new();
        };
        c.identity = Some(feishu.app_id.clone());
        let (pointer, secret_ok) = pointer_of(e, &feishu.app_secret, &feishu.app_secret);
        c.secret_pointer = Some(pointer);
        if !secret_ok {
            c.state = CapabilityState::Unauthorized;
            c.detail = Some("the app secret was not found".into());
        } else if feishu.identity().is_ok_and(|i| i == "user") {
            match jeeves_connector::feishu_oauth::status(e.home) {
                None => {
                    c.state = CapabilityState::Unauthorized;
                    c.detail = Some("no user sign-in: run `jeeves connector feishu login`".into());
                }
                Some((true, _)) => {
                    c.state = CapabilityState::Expired;
                    c.detail = Some("the user sign-in has expired".into());
                }
                Some(_) => {}
            }
        }
        if c.state == CapabilityState::Ready {
            if feishu.allow_recipients.is_empty() {
                c.state = CapabilityState::BlockedByPolicy;
                c.detail = Some("the recipient allowlist is empty".into());
            } else if !feishu.live {
                c.detail = Some("drafts only (live is off)".into());
            }
        }
        vec![c]
    }
}

pub(crate) struct CalendarProbe;

impl CapabilityProbe for CalendarProbe {
    fn cards(&self, e: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let mut c = card(
            "connector:calendar".into(),
            CapabilityKind::Connector,
            "Calendar".into(),
            "calendar_list".into(),
            e.now,
        );
        c.manifest = Some(jeeves_mcp::manifest::builtin("calendar").clone());
        let config = match jeeves_connector::config::load(e.home) {
            Ok(cfg) => cfg,
            Err(error) => {
                c.state = CapabilityState::Unknown;
                c.detail = Some(error.to_string());
                return vec![c];
            }
        };
        let Some(calendar) = config.calendar else {
            return Vec::new();
        };
        c.identity = Some(calendar.timezone.clone());
        if let Some(password) = calendar.password.as_deref() {
            let (pointer, present) = pointer_of(e, password, password);
            c.secret_pointer = Some(pointer);
            if !present {
                c.state = CapabilityState::Unauthorized;
                c.detail = Some("the calendar password was not found".into());
            }
        }
        vec![c]
    }
}

pub(crate) struct EmailProbe;

impl CapabilityProbe for EmailProbe {
    fn cards(&self, e: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let mut c = card(
            "connector:email".into(),
            CapabilityKind::Connector,
            "Email".into(),
            "email_read,email_send".into(),
            e.now,
        );
        c.manifest = Some(jeeves_mcp::manifest::builtin("email").clone());
        let config = match jeeves_connector::config::load(e.home) {
            Ok(cfg) => cfg,
            Err(error) => {
                c.state = CapabilityState::Unknown;
                c.detail = Some(error.to_string());
                return vec![c];
            }
        };
        let Some(email) = config.email else {
            return Vec::new();
        };
        c.identity = Some(email.username.clone());
        let (pointer, present) = pointer_of(e, &email.password, &email.password);
        c.secret_pointer = Some(pointer);
        if !present {
            c.state = CapabilityState::Unauthorized;
            c.detail = Some("the email password was not found".into());
        } else if email.allow_recipients.is_empty() {
            c.detail = Some("read only: the send allowlist is empty".into());
        } else if !email.live {
            c.detail = Some("reads now; sends write drafts only (live is off)".into());
        }
        vec![c]
    }
}

/// The in-memory cache: card id -> (probed at, card).
#[derive(Default, Clone)]
pub(crate) struct Cards {
    cache: std::sync::Arc<Mutex<CardCache>>,
}

#[derive(Default)]
struct CardCache {
    cards: HashMap<String, (i64, CapabilityCard)>,
    user_mcp_digest: Option<String>,
}

impl Cards {
    pub(crate) fn invalidate(&self) {
        self.cache
            .lock()
            .expect("capability cache lock")
            .cards
            .clear();
    }
    /// Probe everything not cached within [`CACHE_SECS`]; fresh entries are returned as they are.
    pub(crate) fn refresh(&self, env: &mut ProbeEnv<'_>) -> Vec<CapabilityCard> {
        let probes: [&dyn CapabilityProbe; 5] = [
            &ModelProbe,
            &McpProbe,
            &FeishuProbe,
            &CalendarProbe,
            &EmailProbe,
        ];
        let digest = std::fs::read(env.home.join("mcp.json"))
            .ok()
            .map(|bytes| jeeves_state::ledger::digest_of(&bytes));
        let mut state = self.cache.lock().expect("capability cache lock");
        if state.user_mcp_digest != digest {
            state.cards.clear();
            state.user_mcp_digest = digest;
        }
        let cache = &mut state.cards;
        let now = env.now;
        let fresh = |cache: &HashMap<String, (i64, CapabilityCard)>, id: &str| {
            cache
                .get(id)
                .filter(|(at, _)| now - at < CACHE_SECS)
                .map(|(_, c)| c.clone())
        };
        let all_fresh =
            !cache.is_empty() && cache.values().all(|(at, _)| env.now - at < CACHE_SECS);
        if all_fresh {
            let mut cards: Vec<_> = cache.values().map(|(_, c)| c.clone()).collect();
            cards.sort_by(|a, b| a.id.cmp(&b.id));
            return cards;
        }
        let mut out = Vec::new();
        let mut keep = HashMap::new();
        for probe in probes {
            for c in probe.cards(env) {
                let c = fresh(cache, &c.id).unwrap_or(c);
                let at = c.last_checked_at.map_or(env.now, i64::from);
                keep.insert(c.id.clone(), (at, c.clone()));
                out.push(c);
            }
        }
        *cache = keep;
        out
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl crate::Server {
    fn probe_all(&self) -> Vec<CapabilityCard> {
        let home = self.settings().home;
        let models = self.models.read().expect("models lock").clone();
        let connected: Vec<String> = self.mcp.servers();
        let missing_scope = self.mcp.missing_scope();
        let extra = std::env::var_os("JEEVES_MCP_CONFIG");
        let mut vault = jeeves_config::models::SecretStore::auto();
        let lookup = |name: &str| std::env::var(name).ok();
        let mut env = ProbeEnv {
            home: &home,
            now: now_secs(),
            env: &lookup,
            vault: &mut vault,
            models: &models,
            connected_mcp: &connected,
            missing_scope_mcp: &missing_scope,
            mcp_extra: extra.as_deref(),
        };
        self.capabilities.refresh(&mut env)
    }

    pub(crate) fn capability_list(&self) -> jeeves_protocol::CapabilityListResponse {
        jeeves_protocol::CapabilityListResponse {
            cards: self.probe_all(),
        }
    }

    pub(crate) fn capability_probe(
        &self,
        id: &str,
    ) -> Result<jeeves_protocol::CapabilityProbeResponse, String> {
        self.probe_all()
            .into_iter()
            .find(|c| c.id == id)
            .map(|card| jeeves_protocol::CapabilityProbeResponse { card })
            .ok_or_else(|| format!("no capability with id {id:?}"))
    }
}

#[cfg(test)]
#[path = "capability_tests.rs"]
mod capability_tests;
