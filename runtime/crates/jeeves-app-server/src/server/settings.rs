use crate::Server;
use anyhow::Result;
use jeeves_config::Settings;
use jeeves_protocol::{SettingsPatch, SettingsView};
use jeeves_provider::Resolved;
use jeeves_state::{Chat, Episodes, Facts};
use sqlx::sqlite::SqlitePool;
use std::sync::Arc;

impl Server {
    /// The state-db connection pool (the scheduler's state goes into this same db — one dataset, no separate store). Clone is cheap.
    pub fn state_pool(&self) -> SqlitePool {
        self.pool.clone()
    }

    /// The notifier (M3a). Used by the scheduler for job-completion delivery; None when unconfigured.
    pub fn notifier(&self) -> Option<Arc<dyn jeeves_tools::notify::Notifier>> {
        self.notifier.clone()
    }

    /// A snapshot of the current settings. The copy you get can be used for any length of time —
    /// concurrent writes only affect the next turn.
    pub fn settings(&self) -> Settings {
        self.settings
            .read()
            .expect("the settings lock should not be poisoned")
            .clone()
    }

    pub fn facts(&self) -> &Facts {
        &self.facts
    }

    pub fn episodes(&self) -> &Episodes {
        &self.episodes
    }

    pub fn chat(&self) -> &Chat {
        &self.chat
    }

    /// This session's deferred-tool activation set, minted on first use.
    ///
    /// Shared with the turn's tool view (`session_view_with`) so `find_tools` in one turn and
    /// `schemas()` in the next read the same cell.
    pub(crate) fn activation_set(&self, session_id: &str) -> jeeves_tools::ActivationSet {
        let mut sets = match self.activations.lock() {
            Ok(sets) => sets,
            Err(poisoned) => poisoned.into_inner(),
        };
        sets.entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::Mutex::new(Default::default())))
            .clone()
    }

    /// A snapshot of the current provider resolution (None when no key is configured).
    pub fn resolved(&self) -> Option<Resolved> {
        self.resolved
            .read()
            .expect("the resolved lock should not be poisoned")
            .clone()
    }

    pub(crate) fn set_settings(&self, settings: Settings) {
        *self
            .settings
            .write()
            .expect("the settings lock should not be poisoned") = settings;
    }

    pub(crate) fn set_resolved(&self, resolved: Option<Resolved>) {
        *self
            .resolved
            .write()
            .expect("the resolved lock should not be poisoned") = resolved;
    }

    /// Inject a provider resolution. The injection point for eval and tests: the normal path goes
    /// through `jeeves_provider::resolve` (env vars supply the key); eval needs a scripted model to
    /// pin harness behavior offline and deterministically — that is when one is plugged in from outside.
    pub fn install_provider(&self, resolved: Resolved) {
        self.set_resolved(Some(resolved));
    }

    /// Restrict an embedded host's tool surface. This can only remove capabilities.
    pub fn restrict_tools(&mut self, allowed: &[&str]) {
        let names: Vec<String> = self.tools.names().into_iter().map(str::to_string).collect();
        let mut tools = (*self.tools).clone();
        for name in names {
            if !allowed.contains(&name.as_str()) {
                tools = tools.without(&name);
            }
        }
        self.tools = Arc::new(tools);
    }

    /// The body of `config/write`: validate patch → accumulate and persist → apply → re-resolve provider.
    ///
    /// A validation failure returns an error with the reason and changes nothing — a patch is all-or-nothing;
    /// half a patch is worse than none.
    pub(crate) fn apply_config_patch(&self, patch: &SettingsPatch) -> Result<SettingsView, String> {
        validate_patch(patch)?;

        let _profiles = self.models.write().expect("models lock");
        let home = self.settings().home;
        let mut saved = jeeves_config::read_patch(&home)
            .map_err(|e| format!("could not read settings.json: {e}"))?;
        saved.merge_newer(patch);
        jeeves_config::save_patch(&home, &saved)
            .map_err(|e| format!("could not write settings.json: {e}"))?;

        let mut settings = self.settings();
        jeeves_config::apply_patch(&saved, &mut settings);

        let resolved = match jeeves_provider::resolve_with(&settings, &self.catalog) {
            Ok(resolved) => Some(resolved),
            Err(reason) => {
                eprintln!(
                    "(jeeves) after config/write no model is configured; turn/* unavailable — {reason}"
                );
                None
            }
        };
        if patch.exec_allow.is_some() {
            *self
                .exec_allow
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = settings.exec_allow.clone();
        }
        self.set_settings(settings.clone());
        self.set_resolved(resolved);
        Ok(settings.view())
    }
}

/// This used to also check "the provider must exist" — now provider and models belong to
/// `<home>/models.json` (change them via `model/*`, validated in models.rs), so the patch
/// only carries runtime parameters and one bounds table is enough.
pub(crate) fn validate_patch(patch: &SettingsPatch) -> Result<(), String> {
    jeeves_config::validate_patch_values(patch)
}
