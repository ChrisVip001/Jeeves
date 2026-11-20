//! The **runtime** of multiple model profiles: `model/profiles` / `model/save` / `model/remove`.
//!
//! Storage and the credentials backend live in `jeeves-config`'s models.rs; this module handles three
//! things: validation, applying to the current Settings, and re-resolving the provider. **Takes effect
//! immediately**, no restart — that is the whole point of "multi-model switching".
//!
//! One discipline: **only a human can call these methods**. The model has no corresponding tools and
//! cannot reach the keys — if it could make Jeeves switch providers itself, it could plug itself into another server.

use jeeves_config::models as cfg;
use jeeves_config::routes::{load_routes, save_routes, RouteFile, RouteMode, TIERS};
use jeeves_protocol::{
    ApprovalLogEntry, ApprovalLogResponse, ModelProfileView, ModelProfilesResponse,
    ModelRemoveParams, ModelRemoveResponse, ModelRoutesResponse, ModelRoutesSaveParams,
    ModelSaveParams, ModelSaveResponse, ScheduleChangeResponse, ScheduleJob, ScheduleListResponse,
    ScheduleRemoveParams, ScheduleSaveParams, TierRoute, UsageByModel, UsageSummaryParams,
    UsageSummaryResponse,
};
use std::sync::{Mutex, RwLock};

use crate::Server;

/// The two pieces of state the Server holds: the config itself + the credentials backend.
pub(crate) type Models = Arc<RwLock<cfg::ModelsFile>>;
pub(crate) type Secrets = Arc<Mutex<cfg::SecretStore>>;

use std::sync::Arc;

pub(crate) fn new_models(file: cfg::ModelsFile) -> Models {
    Arc::new(RwLock::new(file))
}

pub(crate) fn new_secrets() -> Secrets {
    Arc::new(Mutex::new(cfg::SecretStore::auto()))
}

fn views(file: &cfg::ModelsFile) -> Vec<ModelProfileView> {
    let active = file.active.as_deref();
    file.profiles
        .iter()
        .map(|p| ModelProfileView {
            id: p.id.clone(),
            provider: p.provider.clone(),
            model: p.model.clone(),
            small_model: p.small_model.clone(),
            base_url: p.base_url.clone(),
            key_source: p.key_source.clone(),
            vision: p.vision,
            capabilities: p.capabilities.clone(),
            active: active == Some(p.id.as_str()),
        })
        .collect()
}

/// The routing table's wire shape. Rows always come in `TIERS` order so the panel and the file agree.
fn routes_view(file: &RouteFile) -> ModelRoutesResponse {
    ModelRoutesResponse {
        mode: match file.mode {
            RouteMode::Fixed => "fixed",
            RouteMode::Auto => "auto",
        }
        .to_string(),
        routes: TIERS
            .iter()
            .map(|tier| TierRoute {
                tier: (*tier).to_string(),
                profile_id: file.routes.get(*tier).cloned(),
            })
            .collect(),
    }
}

/// `provider / model`: the one-liner for "which model is current" in the UI.
fn current_label(settings: &jeeves_config::Settings) -> Option<String> {
    let provider = &settings.provider;
    if provider.is_empty() {
        return None;
    }
    Some(match settings.model.as_deref() {
        Some(model) => format!("{provider} / {model}"),
        None => provider.to_string(),
    })
}

/// Patch semantics for fields: given means use it (empty string = clear), absent means keep the old.
fn pick(incoming: Option<String>, existing: Option<String>) -> Option<String> {
    match incoming {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(value),
        None => existing,
    }
}

fn provider_entry<'a>(
    catalog: &'a jeeves_provider::catalog::Catalog,
    provider: &str,
) -> Result<&'a jeeves_provider::catalog::CatalogEntry, String> {
    catalog.lookup(provider).ok_or_else(|| format!(
        "unknown provider '{provider}'. Select a supported wire provider ({}) for a custom endpoint, or define the provider in models-catalog.json.",
        catalog.ids().join(", ")
    ))
}

impl Server {
    /// Every declared job (stored and skill-owned) with its runtime state and next run. A failed state
    /// read does not blank the list: the declarations are what a human can act on, and the reason the
    /// state is missing rides along in `error`.
    pub async fn schedule_list(&self) -> ScheduleListResponse {
        let home = self.settings().home.clone();
        let mut error: Option<String> = None;
        let state = match jeeves_state::Schedules::new(self.pool.clone()).list().await {
            Ok(rows) => rows,
            Err(e) => {
                error = Some(format!("the schedule state could not be read: {e}"));
                Vec::new()
            }
        };
        let stored = match jeeves_state::ScheduleJobs::new(self.pool.clone())
            .list()
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                error = Some(format!("the stored jobs could not be read: {e}"));
                Vec::new()
            }
        };
        let now = chrono::Local::now().naive_local();
        let mut jobs: Vec<ScheduleJob> = Vec::new();
        let mut push = |name: String,
                        cron: String,
                        prompt: String,
                        delivery: String,
                        catch_up: bool,
                        skill: Option<String>| {
            let row = state.iter().find(|row| row.name == name);
            let (next_run, next_run_note) = match jeeves_agent::schedule::next_run_at(&cron, now) {
                Ok(when) => (Some(when), None),
                Err(why) => (None, Some(why)),
            };
            jobs.push(ScheduleJob {
                source: if skill.is_some() { "skill" } else { "store" }.to_string(),
                skill,
                last_fired_minute: row.and_then(|r| r.last_fired_minute.clone()),
                claimed_minute: row.and_then(|r| r.claimed_minute.clone()),
                next_run,
                next_run_note,
                name,
                cron,
                prompt,
                delivery,
                catch_up,
            });
        };
        for skill in jeeves_memory::skills::loaded_skills(&home) {
            let Some(cron) = skill.schedule.clone() else {
                continue;
            };
            push(
                skill.name.clone(),
                cron,
                format!(
                    "Run skill '{}' once the way it says: {}",
                    skill.name, skill.description
                ),
                "outbox".to_string(),
                true,
                Some(skill.name.clone()),
            );
        }
        for job in stored {
            push(
                job.name,
                job.cron,
                job.prompt,
                job.delivery,
                job.catch_up,
                None,
            );
        }
        jobs.sort_by(|left, right| left.name.cmp(&right.name));
        ScheduleListResponse { jobs, error }
    }

    /// Add or replace one stored job. The refusal text is written for the panel and the model.
    pub async fn schedule_save(
        &self,
        p: ScheduleSaveParams,
    ) -> Result<ScheduleChangeResponse, String> {
        let body = jeeves_agent::schedule::JobBody {
            name: p.name,
            cron: p.cron,
            prompt: p.prompt,
            delivery: p.delivery.unwrap_or_default(),
            catch_up: p.catch_up,
        };
        let home = self.settings().home.clone();
        jeeves_tools::schedules::save_job(&home, &self.pool, &body)
            .await
            .map(|message| ScheduleChangeResponse { message })
    }

    /// Delete one stored job (skill-owned jobs are refused with the way out).
    pub async fn schedule_remove(
        &self,
        p: ScheduleRemoveParams,
    ) -> Result<ScheduleChangeResponse, String> {
        let home = self.settings().home.clone();
        jeeves_tools::schedules::remove_job(&home, &self.pool, &p.name)
            .await
            .map(|message| ScheduleChangeResponse { message })
    }

    /// Approval audit (migration 0011): the last 100 "asked before running" records, newest first.
    /// A read failure returns an empty list with a note — the same stance as usage_summary.
    pub async fn approval_log(&self) -> ApprovalLogResponse {
        match self.approval_log.recent(100).await {
            Ok(rows) => ApprovalLogResponse {
                entries: rows
                    .into_iter()
                    .map(|row| ApprovalLogEntry {
                        turn_id: row.turn_id,
                        request_id: row.request_id,
                        tool: row.tool,
                        args_preview: row.args_preview,
                        reason: row.reason,
                        outcome: row.outcome,
                        actor: row.actor,
                        decided_at: row.decided_at,
                    })
                    .collect(),
            },
            Err(e) => {
                eprintln!("(jeeves) the approval audit could not be read: {e}");
                ApprovalLogResponse {
                    entries: Vec::new(),
                }
            }
        }
    }

    /// Usage summary (migration 0010): per-model aggregation + totals. Bookkeeping only, never enforced —
    /// no quota, no rate limit; in BYOK mode the money sits on the user's own vendor bill. In hub mode this
    /// is per-user accounting (one home, one ledger per user). A read failure returns an empty summary with
    /// a note — an error would turn a "look at numbers" method into a "break a whole turn" method.
    pub async fn usage_summary(&self, p: UsageSummaryParams) -> UsageSummaryResponse {
        match self
            .usage
            .summary_between(p.since.as_deref(), p.until.as_deref())
            .await
        {
            Ok(summary) => UsageSummaryResponse {
                turns: summary.turns,
                input_tokens: summary.input_tokens,
                output_tokens: summary.output_tokens,
                by_model: summary
                    .by_model
                    .into_iter()
                    .map(|row| UsageByModel {
                        provider: row.provider,
                        model: row.model,
                        turns: row.turns,
                        input_tokens: row.input_tokens,
                        output_tokens: row.output_tokens,
                    })
                    .collect(),
            },
            Err(e) => {
                eprintln!("(jeeves) the usage rollup could not be read: {e}");
                UsageSummaryResponse {
                    turns: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    by_model: Vec::new(),
                }
            }
        }
    }

    pub fn model_profiles(&self) -> ModelProfilesResponse {
        let file = self
            .models
            .read()
            .expect("the models lock should not be poisoned");
        ModelProfilesResponse {
            active_id: file.active.clone(),
            profiles: views(&file),
        }
    }

    fn read_models_for_edit(
        &self,
        home: &std::path::Path,
        live: &mut cfg::ModelsFile,
    ) -> Result<(cfg::ModelsFile, Option<String>), String> {
        let mut file =
            cfg::read_models(home).map_err(|e| format!("could not read models.json: {e}"))?;
        let recovery = cfg::recover_change(
            home,
            &mut file,
            &mut self.secrets.lock().expect("secrets lock"),
        );
        if !matches!(recovery, Ok(None)) {
            *live = file.clone();
            self.reapply(live);
        }
        Ok((file, recovery?))
    }

    /// Add/update a profile; when `make_active` (default true), switch to it as well. Sending only `id` +
    /// `make_active` means "switch" — the user must not be forced to re-send a key just to switch.
    pub fn model_save(&self, p: ModelSaveParams) -> Result<ModelSaveResponse, String> {
        let id = p.id.trim().to_string();
        if id.is_empty() {
            return Err(
                "id must not be empty (give it a short name, e.g. ds, acme-openai)".to_string(),
            );
        }

        let mut live = self
            .models
            .write()
            .expect("the models lock should not be poisoned");
        let home = self.settings().home;
        let (mut file, _) = self.read_models_for_edit(&home, &mut live)?;
        let existing = cfg::find(&file, &id).cloned();

        let provider = match (p.provider.as_deref(), existing.as_ref()) {
            (Some(v), _) => v.trim().to_string(),
            (None, Some(e)) => e.provider.clone(),
            (None, None) => {
                return Err(format!(
                    "the first configuration of '{id}' needs a provider (model/list has each provider's id)"
                ))
            }
        };
        let entry = provider_entry(&self.catalog, &provider)?;
        let model = match p.model.as_deref() {
            Some(value) if value.trim().is_empty() => entry.model.as_str(),
            Some(value) => value,
            None => existing
                .as_ref()
                .and_then(|profile| profile.model.as_deref())
                .unwrap_or(&entry.model),
        };
        let capabilities = p.capabilities.as_ref().or_else(|| {
            existing
                .as_ref()
                .and_then(|profile| profile.capabilities.as_ref())
        });
        jeeves_provider::capabilities::validate(capabilities, entry.wire, model)?;

        let key = p
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let env = p
            .key_env
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let existing_fields = existing.unwrap_or(cfg::ModelProfile {
            id: id.clone(),
            provider: provider.clone(),
            model: None,
            small_model: None,
            base_url: None,
            key_source: None,
            credential_slot: Default::default(),
            vision: None,
            capabilities: None,
        });
        let profile = cfg::ModelProfile {
            id: id.clone(),
            provider,
            model: pick(p.model, existing_fields.model),
            small_model: pick(p.small_model, existing_fields.small_model),
            base_url: pick(p.base_url, existing_fields.base_url),
            key_source: env
                .map(|var| format!("env:{var}"))
                .or(existing_fields.key_source),
            credential_slot: if env.is_some() {
                Default::default()
            } else {
                existing_fields.credential_slot
            },
            vision: p.vision.or(existing_fields.vision),
            capabilities: p.capabilities.or(existing_fields.capabilities),
        };
        let key_stored_in = cfg::save_profile(
            &home,
            &mut file,
            &mut self.secrets.lock().expect("secrets lock"),
            profile,
            key,
            p.make_active.unwrap_or(true),
        )?;
        let active = file.active.as_deref() == Some(id.as_str());
        let active_id = file.active.clone();
        *live = file;
        let current = self.reapply(&live);

        Ok(ModelSaveResponse {
            id,
            active,
            active_id,
            key_stored_in,
            current,
        })
    }

    pub fn model_remove(&self, p: ModelRemoveParams) -> Result<ModelRemoveResponse, String> {
        let home = self.settings().home;
        let mut live = self
            .models
            .write()
            .expect("the models lock should not be poisoned");
        let (mut file, finished) = self.read_models_for_edit(&home, &mut live)?;
        let removed = if cfg::find(&file, &p.id).is_some() {
            let result = cfg::remove_profile(
                &home,
                &mut file,
                &mut self.secrets.lock().expect("secrets lock"),
                &p.id,
            );
            *live = file;
            self.reapply(&live);
            result?
        } else {
            finished.as_deref() == Some(&p.id)
        };
        Ok(ModelRemoveResponse {
            removed,
            active_id: live.active.clone(),
        })
    }

    /// The routing table (`model/routes`). Always three rows in `TIERS` order — the panel has
    /// nothing to sort or invent, and "no file" is honestly `fixed` + three nulls.
    pub fn model_routes(&self) -> ModelRoutesResponse {
        routes_view(&load_routes(&self.settings().home))
    }

    /// Replace the whole table (`model/routes/save`). Validated **before** writing: a typo in a tier
    /// name or a profile id that does not exist would otherwise only surface as a silent fallback
    /// on some later turn, which is exactly the kind of "it just picked another model" nobody can debug.
    pub fn model_routes_save(
        &self,
        p: ModelRoutesSaveParams,
    ) -> Result<ModelRoutesResponse, String> {
        let mode = match p.mode.trim() {
            "fixed" => RouteMode::Fixed,
            "auto" => RouteMode::Auto,
            other => return Err(format!("mode must be fixed or auto — got '{other}'")),
        };
        let known: Vec<String> = {
            let file = self
                .models
                .read()
                .expect("the models lock should not be poisoned");
            file.profiles
                .iter()
                .map(|profile| profile.id.clone())
                .collect()
        };
        let mut routes = std::collections::HashMap::new();
        for entry in p.routes {
            let tier = entry.tier.trim();
            if !TIERS.contains(&tier) {
                return Err(format!(
                    "the tier must be one of {} — got '{tier}'",
                    TIERS.join(" / ")
                ));
            }
            let Some(id) = entry
                .profile_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if !known.iter().any(|known| known == id) {
                return Err(format!(
                    "the routing table references '{id}', which does not exist — configure that profile before attaching it to a tier"
                ));
            }
            routes.insert(tier.to_string(), id.to_string());
        }
        let file = RouteFile { mode, routes };
        save_routes(&self.settings().home, &file)
            .map_err(|e| format!("could not write model-routes.json: {e}"))?;
        Ok(routes_view(&file))
    }

    /// Apply the active profile into Settings and **re-resolve** the provider: switching takes effect immediately.
    /// A resolution failure is not fatal — record the reason; the turn will spell out "missing key".
    fn reapply(&self, file: &cfg::ModelsFile) -> Option<String> {
        let mut settings = self.settings();
        {
            let home = settings.home.clone();
            let mut store = self
                .secrets
                .lock()
                .expect("the secrets lock should not be poisoned");
            if let Some(profile) = cfg::active_profile(file) {
                cfg::apply_profile(profile, &home, &mut *store, &mut settings);
            } else {
                let boot = &self.boot_settings;
                settings.provider.clone_from(&boot.provider);
                settings.model.clone_from(&boot.model);
                settings.small_model.clone_from(&boot.small_model);
                settings.base_url.clone_from(&boot.base_url);
                settings.api_key.clone_from(&boot.api_key);
                settings.vision = boot.vision;
                settings
                    .model_capabilities
                    .clone_from(&boot.model_capabilities);
            }
        }
        let resolved = match jeeves_provider::resolve_with(&settings, &self.catalog) {
            Ok(resolved) => Some(resolved),
            Err(reason) => {
                eprintln!("(jeeves) after switching models nothing is configured yet; turn/* unavailable — {reason}");
                None
            }
        };
        let label = current_label(&settings);
        self.set_settings(settings);
        self.set_resolved(resolved);
        self.route_cache
            .lock()
            .expect("the route-cache lock should not be poisoned")
            .clear();
        label
    }
}

#[cfg(test)]
#[path = "models_tests.rs"]
mod models_tests;
