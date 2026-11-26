//! `jeeves-tools` — the tool registry.
//!
//! A tool is three things: a model-readable name + description, a JSON schema for
//! arguments, and a Rust function that actually runs it. That's all.
//!
//! **On failure, return the error as text to the model instead of crashing the loop.**
//! Seeing the error, the model can take another path or explain to the user — far
//! better than failing the whole turn.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;
use jeeves_provider::ToolSchema;
pub use jeeves_state::operations::ToolOutput;
use jeeves_state::{Chat, Episodes, Facts};
use serde_json::Value;

pub mod approval;
mod arg_coerce;
pub mod browser;
mod code_map;
pub mod computer_use;
pub mod edit_checks;
pub mod edit_match;
pub mod exec;
pub mod file_seen;
pub mod fs;
pub mod fs_write;
pub mod handlers;
pub mod jobs;
pub mod memory_admin;
pub mod memory_tool;
pub mod messages;
pub mod notify;
mod notify_queue;
pub mod patch;
pub mod permissions;
pub mod report;
pub mod resources;
pub mod roots;
pub mod schedules;
pub mod search;
pub mod snapshot;
pub mod spill;
pub mod subagent;
pub mod todo;
mod tool_name;
pub mod web;

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tools_tests;

#[cfg(test)]
#[path = "exec_tests.rs"]
mod exec_tests;

#[cfg(test)]
#[path = "roots_tests.rs"]
mod roots_tests;

#[cfg(test)]
#[path = "subagent_tests.rs"]
mod subagent_tests;

#[cfg(test)]
#[path = "approval_tests.rs"]
mod approval_tests;

#[cfg(test)]
#[path = "spill_tests.rs"]
mod spill_tests;

#[cfg(test)]
#[path = "report_tests.rs"]
mod report_tests;

#[cfg(test)]
#[path = "notify_tests.rs"]
mod notify_tests;

#[cfg(test)]
#[path = "todo_tests.rs"]
mod todo_tests;

#[cfg(test)]
#[path = "fs_tests.rs"]
mod fs_tests;

#[cfg(test)]
#[path = "fs_write_tests.rs"]
mod fs_write_tests;

#[cfg(test)]
#[path = "edit_match_tests.rs"]
mod edit_match_tests;

#[cfg(test)]
#[path = "patch_tests.rs"]
mod patch_tests;

#[cfg(test)]
#[path = "web_tests.rs"]
mod web_tests;

#[cfg(test)]
#[path = "search_tests.rs"]
mod search_tests;

/// Host notification that a tool cannot establish its external outcome.
pub type UncertainSink = Arc<dyn Fn(&str) + Send + Sync>;

/// The host environment available to tool handlers.
#[derive(Clone)]
pub struct ToolCtx {
    /// Host-only signal that an external operation may have committed without a receipt.
    pub on_uncertain: Option<UncertainSink>,
    /// Host-acquired mutation permit, shared with any background child.
    pub resource_lease: Option<resources::Lease>,
    /// Canonical, host-assigned Git worktree for an isolated coding task.
    pub worktree: Option<PathBuf>,
    /// Execution identity assigned by the host; never supplied by tool arguments.
    pub task_run_id: Option<String>,
    /// Checkpoint round and tool invocation supplied by the loop, stable on resume.
    pub invocation_id: Option<String>,
    /// Checks that run after each edit (host-implemented). `None` = none declared.
    pub edit_checks: Option<Arc<dyn edit_checks::EditChecks>>,
    pub facts: Facts,
    pub episodes: Episodes,
    pub chat: Chat,
    pub home: PathBuf,
    /// This turn's working directory (session-level working-directory). None = process cwd.
    /// exec's fallback chain: args.cwd > cwd > process cwd; read_file/list_dir resolve against it too.
    pub cwd: Option<PathBuf>,
    /// Which session this turn belongs to. Logs and spills attribute by it (a subagent gets
    /// its own name, so "who produced this oversized result" stays traceable).
    pub session_id: String,
    /// Is there someone to ask for approval? `None` = no (subagent, `JEEVES_APPROVAL=never`,
    /// or a caller with no interactive surface) — actions needing approval are denied
    /// outright, per `approval.rs`'s "deny by default".
    pub approval: Option<Arc<dyn crate::approval::ApprovalBroker>>,
    /// Background job host. `None` = off (it toggles with `JEEVES_EXEC`).
    pub jobs: Option<Arc<dyn crate::jobs::JobRegistry>>,
    /// Host-controlled desktop window. `None` = no helper installed.
    pub computer_use: Option<Arc<dyn crate::computer_use::ComputerUseHost>>,
    /// This turn's web-search budget (`web.rs::TurnLimits`). Created once per turn
    /// and shared by every clone of the ctx, so the counter survives across the
    /// turn's tool calls; a fresh turn gets a fresh budget.
    pub turn_limits: Arc<crate::web::TurnLimits>,
    /// How `find_tools` widens this session's tool table (`JEEVES_TOOL_POLICY=deferred`).
    /// `None` = the table is complete already, and the tool says so.
    pub activate: Option<Activator>,
    /// Live output while a tool is still running (`run_command` pipes its stdout here).
    ///
    /// `None` = nobody is watching (CLI, eval, a background job) — the tool just collects and
    /// returns. The final result is **always** the tool's return value; deltas are a progress
    /// feed for a human, never the answer, so a dropped chunk costs nothing but a repaint.
    pub on_delta: Option<DeltaSink>,
}

/// One chunk of live tool output. `Fn` and not a stream: the consumer is a notification sink
/// (SSE) that must not block the tool, and backpressure on a human's screen is meaningless.
pub type DeltaSink = Arc<dyn Fn(&str) + Send + Sync>;

pub type BoxFut = Pin<Box<dyn std::future::Future<Output = Result<String>> + Send>>;
pub type Handler = Arc<dyn Fn(ToolCtx, Value) -> BoxFut + Send + Sync>;

pub use jeeves_protocol::ToolEffect;

#[derive(Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub handler: Handler,
}

/// The schema shape must match what the provider expects; convert once, here.
impl Tool {
    pub fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
        }
    }
}

/// Tools that **never** hide behind `find_tools`.
///
/// Two reasons to be on this list. The gated ones (`run_command`, `write_file`, `edit`,
/// `delegate_task`) are capabilities a human switched on — burying them behind a discovery
/// step would make the switch look broken, and hiding them behind a *different* search than
/// the switch is how a permission model rots. The rest are the handful every turn plausibly
/// needs; deferring them costs more in discovery turns than it saves in schema tokens.
pub const ALWAYS_ON: &[&str] = &[
    "run_command",
    "write_file",
    "edit",
    "apply_patch",
    "delegate_task",
    "todo_write",
    "browser_observe",
    "browser_act",
    "browser_batch",
    "find_tools",
    "memory",
    "read_page",
    "read_file",
    "glob",
    "grep",
    "schedule_job",
    "list_schedules",
    "cancel_schedule",
];

/// One session's deferred-tool activation set (`JEEVES_TOOL_POLICY=deferred`).
///
/// A named type because the shape is genuinely nested (session map → set) and the nesting
/// says nothing at the use site: `Arc<Mutex<BTreeSet<String>>>` four times in a signature
/// reads as punctuation, not meaning.
pub type ActivationSet = Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>;

/// Activates deferred tools for one session, by search query. The registry hands it out;
/// `find_tools` calls it. Returns the names it opened (empty = nothing matched).
pub type Activator = Arc<dyn Fn(&str) -> Vec<String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct ToolRegistry {
    policy: Option<Arc<dyn permissions::ExecutionPolicy>>,
    resources: Option<Arc<dyn resources::ResourceCoordinator>>,
    tools: BTreeMap<String, Tool>,
    /// Each tool's **precompiled** argument validator. Compiled once at registration, zero parsing at call time.
    ///
    /// The outer `Option` is "is this tool in the table", the inner is "does its schema compile":
    /// an MCP server may report a broken schema; then skip validation and run anyway —
    /// a broken remote schema must not make a tool unusable (but say so in the startup log;
    /// silently skipping validation makes people think arguments were checked).
    validators: BTreeMap<String, Option<jsonschema::Validator>>,
    /// `JEEVES_TOOL_POLICY=deferred`: names whose schema stays out of the request until this
    /// session activates them. Empty = eager, every schema rides (the default).
    ///
    /// Hiding a schema is a **token** decision, never a permission one: an activated tool is
    /// still subject to its switch and its approval gate, and a deferred tool is still
    /// callable by name — the table is what the model is shown, not what it may do.
    deferred: std::collections::BTreeSet<String>,
    /// Activated names. Per session, hence a shared cell rather than a set on the registry:
    /// the loop holds `&ToolRegistry` while `find_tools` must mutate this through `ToolCtx`.
    activated: Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    /// Tools that must run **alone**. The loop turns each into a barrier: calls before it
    /// finish first, calls after it wait. Writes, exec, subagents and approval-carrying tools
    /// land here, so two writes never interleave and at most one approval card is up.
    exclusive: std::collections::BTreeSet<String>,
    /// Declared external effects. Absent means `Write` (the conservative default).
    effects: BTreeMap<String, ToolEffect>,
    /// Browser write actions are refused before the approval gate. Copied with the registry,
    /// so a subagent inherits the restriction and cannot widen it.
    browser_view_only: bool,
}

impl ToolRegistry {
    pub fn set_execution_policy(&mut self, policy: Arc<dyn permissions::ExecutionPolicy>) {
        self.policy = Some(policy);
    }
    /// Make browser write actions read-only for this registry and every copy of it.
    pub fn set_browser_view_only(&mut self) {
        self.browser_view_only = true;
    }
    /// A view-only registry refuses `browser_act` write kinds before authorization.
    pub fn browser_view_only(&self) -> bool {
        self.browser_view_only
    }
    /// Share one mutation coordinator across session copies of this registry.
    pub fn set_resources(&mut self, coordinator: Arc<dyn resources::ResourceCoordinator>) {
        self.resources = Some(coordinator);
    }

    pub fn new() -> Self {
        Self::default()
    }

    /// Remove one tool's copy (for subagents: they cannot delegate further).
    ///
    /// Clone the whole registry instead of sharing — the subagent's table and the parent's
    /// then never affect each other, and tools are `Arc`-wrapped, so cloning is cheap.
    pub fn without(&self, name: &str) -> Self {
        let mut copy = self.clone();
        copy.tools.remove(name);
        copy.validators.remove(name);
        copy.effects.remove(name);
        copy
    }

    /// A copy with every **exclusive** tool removed — the table a batched subagent gets.
    ///
    /// Why remove instead of serialize: four subagents run at once, each in its own loop, so the
    /// parent's barrier cannot reach them. Two of them writing the same directory means the last
    /// writer wins — silently. Taking the write/exec tools away makes "batch = read-only" a fact
    /// of the tool table rather than a rule someone has to remember.
    ///
    /// Both maps are trimmed together: a validator without a tool (or the reverse) is a table
    /// that half-exists, which is exactly the kind of inconsistency nobody debugs later.
    pub fn readonly(&self) -> Self {
        let mut copy = self.clone();
        for name in &self.exclusive {
            copy.tools.remove(name);
            copy.validators.remove(name);
            copy.effects.remove(name);
        }
        copy.exclusive.clear();
        copy.deferred.retain(|name| copy.tools.contains_key(name));
        copy
    }

    pub fn register(&mut self, tool: Tool) {
        self.register_with_effect(tool, ToolEffect::Write);
    }

    /// Register with an explicit side-effect class. An undeclared tool is a `Write`, never a read.
    pub fn register_with_effect(&mut self, tool: Tool, effect: ToolEffect) {
        if self.tools.contains_key(&tool.name) {
            eprintln!(
                "(jeeves) the tool name '{}' is already taken; the later registration is refused (it does not overwrite the existing one)",
                tool.name
            );
            return;
        }
        let validator = match jsonschema::validator_for(&tool.input_schema) {
            Ok(validator) => Some(validator),
            Err(e) => {
                eprintln!(
                    "(jeeves) tool '{}' has an input_schema that does not compile ({e}) — its arguments are not validated",
                    tool.name
                );
                None
            }
        };
        self.validators.insert(tool.name.clone(), validator);
        self.effects.insert(tool.name.clone(), effect);
        self.tools.insert(tool.name.clone(), tool);
    }

    /// Register a tool that must run **alone** (write / exec / subagent / anything that asks
    /// a human). A read-only tool registered normally may share a batch with other read-only
    /// calls; this one is always a barrier.
    ///
    /// The flag lives on the registry rather than on `Tool` because it is a scheduling
    /// property of this table, not of the implementation: the same MCP tool may be one or the
    /// other depending on where it was mounted.
    pub fn register_exclusive(&mut self, tool: Tool) {
        self.register_exclusive_with_effect(tool, ToolEffect::Write);
    }

    pub fn register_exclusive_with_effect(&mut self, tool: Tool, effect: ToolEffect) {
        self.exclusive.insert(tool.name.clone());
        self.register_with_effect(tool, effect);
    }

    /// Unknown tools are a write: absence of a declaration never widens permission.
    pub fn effect(&self, name: &str) -> ToolEffect {
        self.effects.get(name).copied().unwrap_or(ToolEffect::Write)
    }

    /// Must this call run alone? Unknown names are not exclusive — they fail fast as text,
    /// and a made-up name must not change scheduling.
    pub fn is_exclusive(&self, name: &str) -> bool {
        self.exclusive.contains(name)
            || (self.policy.is_some()
                && !self.effect(name).is_read()
                && !permissions::proven_read(name))
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.get(name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.keys().map(String::as_str).collect()
    }

    /// Defer every tool not on `ALWAYS_ON` (`JEEVES_TOOL_POLICY=deferred`).
    ///
    /// Called once, after the table is assembled: the policy is a shape of the request, not
    /// a property of each registration.
    pub fn defer_all_but_always(&mut self) {
        for name in self.tools.keys() {
            if !ALWAYS_ON.contains(&name.as_str()) {
                self.deferred.insert(name.clone());
            }
        }
    }

    /// A per-session copy: same tools, fresh activation set.
    ///
    /// Per session on purpose — "I showed the model the calendar schema" is a fact about
    /// this conversation, and one session's discovery must not quietly widen another's request.
    pub fn session_view(&self) -> Self {
        self.session_view_with(Arc::new(std::sync::Mutex::new(Default::default())))
    }

    /// A view over an **existing** activation set.
    ///
    /// `find_tools` is called in one turn and its result lands in the next, so the set has
    /// to outlive a single turn. The caller (app-server) owns the per-session cell; a fresh
    /// registry on every turn would make the model rediscover the same tool every message —
    /// strictly worse than the eager table it replaced.
    pub fn session_view_with(&self, activated: ActivationSet) -> Self {
        let mut view = self.clone();
        view.activated = activated;
        view
    }

    /// Hand `find_tools` the ability to activate, without giving it the whole registry.
    ///
    /// Matching is deliberately plain: lowercase substring of the query in the name or the
    /// description. A semantic search over twelve tool descriptions would need a model call
    /// to save a few hundred tokens — the trade is backwards, and "why did this not match"
    /// must stay answerable by reading the code.
    pub fn activator(&self) -> Activator {
        let catalogue: Vec<(String, String)> = self
            .tools
            .values()
            .map(|tool| (tool.name.clone(), tool.description.to_lowercase()))
            .collect();
        let deferred = self.deferred.clone();
        let activated = self.activated.clone();
        Arc::new(move |query: &str| {
            let Ok(mut set) = activated.lock() else {
                return Vec::new();
            };
            let hidden: Vec<&(String, String)> = catalogue
                .iter()
                .filter(|(name, _)| deferred.contains(name) && !set.contains(name))
                .collect();
            let needle = query.to_lowercase();
            let words: Vec<&str> = needle.split_whitespace().collect();
            let mut hit: Vec<String> = hidden
                .iter()
                .filter(|(name, desc)| {
                    words
                        .iter()
                        .any(|word| name.to_lowercase().contains(word) || desc.contains(word))
                })
                .map(|(name, _)| name.clone())
                .collect();
            if hit.is_empty() {
                hit = hidden.iter().map(|(name, _)| name.clone()).collect();
            }
            for name in &hit {
                set.insert(name.clone());
            }
            hit
        })
    }

    /// What the model sees: everything in eager mode, `ALWAYS_ON` + activated in deferred.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        let activated = self
            .activated
            .lock()
            .map(|set| set.clone())
            .unwrap_or_default();
        self.tools
            .values()
            .filter(|tool| !self.deferred.contains(&tool.name) || activated.contains(&tool.name))
            .map(Tool::schema)
            .collect()
    }

    /// Apply the current session policy and writer permit to a tool or a host-run acceptance check.
    pub async fn admit_call(
        &self,
        ctx: &mut ToolCtx,
        name: &str,
        args: &Value,
    ) -> Result<(), String> {
        if self.browser_view_only && name == "browser_act" {
            let kind = args["kind"].as_str();
            if !matches!(kind, Some("navigate" | "scroll")) {
                return Err(
                    "browser_act is view-only for this session; navigate and scroll are allowed, clicks, fills and other writes are refused before approval"
                        .into(),
                );
            }
        }
        if self.browser_view_only && name == "browser_batch" {
            let steps = args["steps"].as_array().ok_or_else(|| {
                "browser_batch requires a steps array even in view-only mode".to_string()
            })?;
            if steps
                .iter()
                .any(|step| !matches!(step["kind"].as_str(), Some("navigate" | "scroll")))
            {
                return Err(
                    "browser_batch is view-only for this session; navigate and scroll steps are allowed, clicks, fills and other writes are refused before approval"
                        .into(),
                );
            }
        }
        let ticket = if let Some(policy) = &self.policy {
            Some(
                policy
                    .authorize(
                        ctx.clone(),
                        name.to_owned(),
                        self.effect(name),
                        args.clone(),
                    )
                    .await?,
            )
        } else {
            None
        };
        if ticket.as_ref().is_some_and(|ticket| ticket.approved) {
            ctx.approval = Some(Arc::new(permissions::ApprovedInvocation));
        }
        let steering = name == "task" && args["action"] == "continue";
        if ctx.resource_lease.is_none()
            && self.is_exclusive(name)
            && name != "delegate_task"
            && !steering
            && let Some(resources) = &self.resources
        {
            ctx.resource_lease = Some(resources.acquire(name, ctx.worktree.as_deref()).await?);
        }
        if let (Some(policy), Some(ticket)) = (&self.policy, ticket) {
            policy.validate(&ctx.session_id, ticket.revision)?;
        }
        Ok(())
    }

    /// Run one call. **Never returns Err** — errors become text the model can read.
    /// Unknown tools too; the model can correct itself.
    pub async fn execute(&self, ctx: ToolCtx, name: &str, args: Value) -> String {
        self.execute_result(ctx, name, args).await.text
    }

    pub async fn execute_result(&self, mut ctx: ToolCtx, name: &str, args: Value) -> ToolOutput {
        let repaired = (!self.tools.contains_key(name))
            .then(|| tool_name::resolve(name, self.tools.keys().map(String::as_str)))
            .flatten();
        if let Some(registered) = &repaired {
            eprintln!(
                "(jeeves) the model called '{name}'; running the registered tool '{registered}'"
            );
        }
        let name = repaired.as_deref().unwrap_or(name);
        let Some(tool) = self.tools.get(name) else {
            return ToolOutput::failure(format!(
                "Error: no tool named '{name}'. Available: {}",
                self.names().join(", ")
            ));
        };

        if let Some(why) = jeeves_provider::malformed_tool_input(&args) {
            return ToolOutput::failure(format!("Error: {name}: {why}"));
        }

        let mut args = args;
        if let Some(Some(validator)) = self.validators.get(name)
            && let Err(why) = describe_violations(validator, &args)
        {
            match arg_coerce::coerce(&tool.input_schema, &args)
                .filter(|(repaired, _)| validator.is_valid(repaired))
            {
                Some((repaired, keys)) => {
                    eprintln!(
                        "(jeeves) {name}: arguments {} arrived as strings; parsed them to the schema's types and ran the call",
                        keys.join(", ")
                    );
                    args = repaired;
                }
                None => {
                    return ToolOutput::failure(format!(
                        "Error: the arguments do not match the {name} schema — {why}. Rewrite the input to satisfy the schema."
                    ));
                }
            }
        }

        if let Err(why) = file_seen::check_call(&ctx, name, &args) {
            return ToolOutput::failure(format!("Error: {name} failed: {why}"));
        }

        if let Err(reason) = self.admit_call(&mut ctx, name, &args).await {
            return ToolOutput::failure(format!("Error: {reason}"));
        }
        match (tool.handler)(ctx, args).await {
            Ok(text) => ToolOutput::success(text),
            Err(e) => ToolOutput::failure(format!("Error: {name} failed: {e}")),
        }
    }
}

/// Validate a JSON value against a schema; the error is the model-facing sentence (`Ok(())` if valid).
///
/// Tool-argument validation goes through the same path (`describe_violations`), and so does
/// the subagent's structured result — the wording is identical, so the model learns one dialect.
pub fn validate_value(schema: &Value, value: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|e| format!("this schema does not compile: {e}"))?;
    describe_violations(&validator, value)
}

/// Translate schema violations into one model-facing sentence: at most three, each with a field path.
///
/// No dumping the whole schema — it would flood the context, and the model only needs "what's wrong".
/// Returns `Ok(())` when everything is valid.
fn describe_violations(validator: &jsonschema::Validator, args: &Value) -> Result<(), String> {
    let all: Vec<String> = validator
        .iter_errors(args)
        .map(|error| {
            let path = error.instance_path();
            if path.is_empty() {
                format!("the arguments themselves: {error}")
            } else {
                format!("{path}: {error}")
            }
        })
        .collect();
    if all.is_empty() {
        return Ok(());
    }
    let shown = all.len().min(3);
    let mut text = all[..shown].join("；");
    if all.len() > shown {
        text.push_str(&format!("({} more)", all.len() - shown));
    }
    Err(text)
}

/// Take a required string argument. Clear errors for missing/wrong type — this text lands
/// in the model's context; reading it, it knows how to fix things.
pub fn require_str(args: &Value, key: &str) -> Result<String> {
    let value = args
        .get(key)
        .ok_or_else(|| anyhow::anyhow!("Missing parameter '{key}'"))?;
    let text = value
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Parameter '{key}' should be a string, got {value}"))?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Parameter '{key}' must not be empty");
    }
    Ok(trimmed.to_string())
}

/// Take an optional integer argument (with a default).
pub fn opt_u32(args: &Value, key: &str, default: u32) -> Result<u32> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v.as_u64().map(|n| n as u32).ok_or_else(|| {
            anyhow::anyhow!("Parameter '{key}' should be a non-negative integer, got {v}")
        }),
    }
}
