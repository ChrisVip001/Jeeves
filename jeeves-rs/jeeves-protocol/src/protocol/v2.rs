//! Jeeves app-server v2 — the contract itself.
//!
//! This single definition feeds the Rust app-server (as-is), the TypeScript frontends (ts-rs)
//! and a JSON Schema copy for non-TypeScript callers (schemars).
//! **Editing this file means editing the protocol**; run `just write-app-server-schema` after.
//!
//! Naming rules:
//!   * `*Params`        request payload, client → server
//!   * `*Response`      response payload, server → client
//!   * `*Notification`  push message, server → client
//!   * camelCase everywhere on the wire; enum values too
//!   * timestamps: Unix seconds (`*_at`) or ISO 8601 strings (human-facing fields)
//!   * **integer fields are always `i32`**, never `i64`: ts-rs maps `i64` to
//!     TypeScript `bigint` while `JSON.parse` yields `number` — a mismatch
//!     that blows up at runtime. Every integer here is a count / id /
//!     milliseconds, well below 2^31; storage may still use i64, narrowed at the boundary.

use serde::{Deserialize, Serialize};

use crate::{JsonSchema, TS};

/// JSON-RPC method names. `<resource>/<method>`, resource in singular.
pub mod methods {
    pub const TURN_OPERATIONS: &str = "turn/operations";
    pub const TURN_RECONCILE: &str = "turn/reconcile";
    pub const TURN_START: &str = "turn/start";
    /// Human interaction entry: route conversation and independently scoped tasks.
    pub const INTERACTION_START: &str = "interaction/start";
    pub const SESSION_MAIN: &str = "session/main";
    pub const TURN_INTERRUPT: &str = "turn/interrupt";
    pub const DESKTOP_STATUS: &str = "desktop/status";
    pub const DESKTOP_SELECT: &str = "desktop/select";
    pub const DESKTOP_OBSERVE: &str = "desktop/observe";
    pub const DESKTOP_ACT: &str = "desktop/act";
    pub const DESKTOP_TAKEOVER: &str = "desktop/takeover";
    pub const DESKTOP_MODEL: &str = "desktop/model";
    pub const DESKTOP_FRAME: &str = "desktop/frame";
    pub const DESKTOP_SHARE: &str = "desktop/share";
    pub const EXECUTOR_CONTROL: &str = "executor/control";
    pub const EXECUTOR_STOP: &str = "executor/stop";
    pub const EXECUTOR_RELEASE: &str = "executor/release";
    pub const DEVICE_PAIR: &str = "device/pair";
    pub const DEVICE_REVOKE: &str = "device/revoke";
    pub const DEVICE_LIST: &str = "device/list";
    pub const DEVICE_REPORT: &str = "device/report";
    pub const BROWSER_LIST: &str = "browser/list";
    pub const BROWSER_START: &str = "browser/start";
    pub const BROWSER_INPUT: &str = "browser/input";
    pub const BROWSER_DOWNLOADS: &str = "browser/downloads";
    pub const BROWSER_DOWNLOAD: &str = "browser/download";
    pub const BROWSER_CONTROL: &str = "browser/control";
    pub const BROWSER_FRAME: &str = "browser/frame";
    pub const APPROVAL_NOTICE: &str = "approval/notice";
    pub const APPROVAL_NOTICE_DECISION: &str = "approval/noticeDecision";
    pub const APPROVAL_PUSH_KEY: &str = "approval/pushKey";
    pub const APPROVAL_PUSH_SUBSCRIBE: &str = "approval/pushSubscribe";
    pub const APPROVAL_PENDING: &str = "approval/pending";
    pub const APPROVAL_RESPOND: &str = "approval/respond";
    /// Read a session's todo sheet (the dashboard's task card). Read-only, human-facing.
    pub const TODO_VIEW: &str = "todo/view";
    /// Long tasks: create / list / view / get / runs / artifacts / suspend / resume / cancel.
    /// **All human-only** — the model has no path to any of them.
    pub const TASK_START: &str = "task/start";
    pub const TASK_OPERATIONS: &str = "task/operations";
    pub const TASK_RECONCILE: &str = "task/reconcile";
    pub const TASK_WAIT: &str = "task/wait";
    pub const TASK_SIGNAL: &str = "task/signal";
    /// An inbound channel message wakes the task waiting for it, once per source key.
    pub const TASK_WAKE: &str = "task/wake";
    pub const TASK_INTEGRATE: &str = "task/integrate";
    pub const TASK_GIT_CHECKPOINT: &str = "task/gitCheckpoint";
    pub const TASK_GIT_CHECKPOINT_POLICY: &str = "task/gitCheckpointPolicy";
    pub const TASK_GIT_CHECKPOINT_POLICY_SET: &str = "task/gitCheckpointPolicy/set";
    pub const TASK_CREATE: &str = "task/create";
    pub const TASK_LIST: &str = "task/list";
    pub const TASK_VIEW: &str = "task/view";
    /// Task events after a cursor, oldest first (the timeline a reconnecting client replays).
    pub const TASK_EVENTS: &str = "task/events";
    pub const TASK_GET: &str = "task/get";
    /// Every capability (model, MCP server, connector) with whether it can be used right now.
    pub const CONNECTOR_TOOLS: &str = "connector/tools";
    pub const CONNECTOR_TOOLS_SET: &str = "connector/tools/set";
    pub const CONNECTOR_CATALOG: &str = "connector/catalog";
    pub const CAPABILITY_LIST: &str = "capability/list";
    /// Re-check one capability card (answered from cache within 60 seconds).
    pub const CAPABILITY_PROBE: &str = "capability/probe";
    pub const CONNECTOR_SETTINGS: &str = "connector/settings";
    pub const CONNECTOR_SETTINGS_SAVE: &str = "connector/settings/save";
    pub const CONNECTOR_OAUTH_START: &str = "connector/oauth/start";
    /// Every attempt of one task (how many times it ran, what each attempt cost).
    pub const TASK_RUNS: &str = "task/runs";
    /// The files this task produced.
    pub const TASK_ARTIFACTS: &str = "task/artifacts";
    pub const TASK_SUSPEND: &str = "task/suspend";
    pub const TASK_RESUME: &str = "task/resume";
    pub const TASK_CANCEL: &str = "task/cancel";
    /// Shells declare what they can do; the engine answers which of those it supports.
    pub const SESSION_CAPABILITIES: &str = "session/capabilities";
    pub const SESSION_PERMISSION_GET: &str = "session/permission/get";
    pub const SESSION_PERMISSION_SET: &str = "session/permission/set";
    pub const SESSION_LIST: &str = "session/list";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_MESSAGES: &str = "session/messages";
    /// One session's own record: name, working directory, message count.
    pub const SESSION_GET: &str = "session/get";
    /// Rename a session or rebind its working directory (`""` clears). **Human-only** —
    /// the model has no path here, exactly like `task/*`.
    pub const SESSION_UPDATE: &str = "session/update";
    /// Copy the last few exchanges of a session into a new one ("try another path").
    pub const SESSION_FORK: &str = "session/fork";
    /// Which turns are running right now, optionally narrowed to one session. The dashboard's
    /// "which round is running" and the CLI's "what is happening" both read from here.
    pub const TURN_LIST: &str = "turn/list";
    pub const TURN_STEER: &str = "turn/steer";
    pub const TURN_FOLLOW_UP: &str = "turn/follow-up";
    pub const MEMORY_SEARCH: &str = "memory/search";
    pub const MEMORY_LIST: &str = "memory/list";
    pub const MEMORY_LIST_EPISODES: &str = "memory/list-episodes";
    pub const MEMORY_VALIDITY: &str = "memory/setValidity";
    pub const MEMORY_PROVENANCE: &str = "memory/provenance";
    pub const MEMORY_REMEMBER: &str = "memory/remember";
    pub const MEMORY_FORGET: &str = "memory/forget";
    pub const CONFIG_READ: &str = "config/read";
    pub const CONFIG_WRITE: &str = "config/write";
    pub const MODEL_LIST: &str = "model/list";
    /// List configured model profiles + the active one. **Human-only to change** — the model cannot.
    pub const MODEL_PROFILES: &str = "model/profiles";
    /// Add/update a profile (switching along the way when `make_active` is set). Passing
    /// only `id` + `make_active` means "switch" — switching must not force re-sending a key.
    pub const MODEL_SAVE: &str = "model/save";
    pub const MODEL_REMOVE: &str = "model/remove";
    /// The routing table (`<home>/model-routes.json`): fixed / auto, and tier → profile id.
    /// **Human-only to change** — the model cannot route itself, exactly like `/model`.
    pub const MODEL_ROUTES: &str = "model/routes";
    pub const MODEL_ROUTES_SAVE: &str = "model/routes/save";
    /// Usage summary (migration 0010): per-model aggregates + totals. **Bookkeeping only, no enforcement** —
    /// no quotas, no rate limits; in BYOK mode the cost lands on the user's own vendor bill.
    pub const USAGE_SUMMARY: &str = "usage/summary";
    /// Approval audit (migration 0011): what the model asked and how it went
    /// (approved/denied/timeout/voided). Lets a hub admin later answer "who approved rm -rf".
    pub const APPROVAL_LOG: &str = "approval/log";
    /// Every declared job (stored and skill-owned) with its runtime state and next run.
    /// Zero-arg method. Feeds the dashboard's schedule panel.
    pub const SCHEDULE_LIST: &str = "schedule/list";
    /// Add or replace one stored job (validated: cron, name, delivery; a skill-owned name is refused).
    pub const SCHEDULE_SAVE: &str = "schedule/save";
    /// Delete one stored job (a skill-owned job is refused with the way out).
    pub const SCHEDULE_REMOVE: &str = "schedule/remove";
    pub const RESPONSIBILITY_REGISTER: &str = "responsibility/register";
    pub const RESPONSIBILITY_REPORTS: &str = "responsibility/reports";
    pub const RESPONSIBILITY_LIST: &str = "responsibility/list";
    pub const RESPONSIBILITY_SET_PAUSED: &str = "responsibility/setPaused";

    /// One session's ledger: the turn list, plus one turn's entries when `turnId` is given.
    pub const MEMORY_LEDGER: &str = "memory/ledger";
    /// The bundle's catalog (`index.md`, the same text the prompt carries).
    pub const MEMORY_INDEX: &str = "memory/index";
    /// One memory page, body and provenance included.
    pub const MEMORY_PAGE: &str = "memory/page";
    /// How the context is spent right now, per section.
    pub const CONTEXT_USAGE: &str = "context/usage";
    /// How often each skill actually fired (the hit log, aggregated).
    pub const SKILL_STATS: &str = "skill/stats";
    /// Compact on demand (`/compact [focus]`): summarize the older turns now, not at the
    /// next natural boundary. Human-triggered, so it is not exposed to the model.
    pub const SESSION_COMPACT: &str = "session/compact";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TokenUsage {
    pub input_tokens: i32,
    pub output_tokens: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum ToolStatus {
    Ok,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ToolCallRecord {
    pub tool: String,
    pub status: ToolStatus,
    pub duration_ms: Option<i32>,
}

/// A "may I execute this?" query.
///
/// After throwing the question to the client the server **waits** (120 s by default);
/// the answer arrives via `approval/respond`. No reply, timeout, or no approval
/// channel at all — everything is **treated as denied**: default-deny is this feature's foundation, not its edge case.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalRequestedNotification {
    pub turn_id: String,
    /// Request id within this turn; echo it back verbatim when answering.
    pub request_id: String,
    /// What wants to run (tool name; currently only `run_command`).
    pub tool: String,
    /// Human-facing action preview: the raw command or an args summary.
    pub args_preview: String,
    /// Why the allow rules did not let it through automatically.
    pub reason: String,
    /// How many milliseconds of silence count as a denial.
    pub expires_in_ms: i32,
    /// Starts at 1 and rises each time the question is re-issued (a restart). Echo it in
    /// `approval/respond`; an answer carrying an older version is refused.
    #[serde(default = "first_version")]
    pub version: i32,
}

fn first_version() -> i32 {
    1
}

/// Live requests belonging to a conversation, including its task executions.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalPendingParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct PendingApprovalView {
    pub session_id: String,
    pub owner_session_id: String,
    /// True when this question outlived a restart: answering it grants the resumed turn a
    /// one-time yes for the same action instead of unblocking a live call.
    #[serde(default)]
    pub restored: bool,
    pub task: Option<AgentTaskView>,
    /// expiresInMs is remaining time at snapshot, not a renewed approval deadline.
    pub request: ApprovalRequestedNotification,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalPendingResponse {
    pub approvals: Vec<PendingApprovalView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalNoticeParams {
    pub device_id: String,
    pub grant_id: String,
    pub generation: i32,
    pub task_id: String,
    pub turn_id: String,
    pub approval_request_id: String,
    pub approval_version: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalNoticeCheckParams {
    #[serde(flatten)]
    pub notice: ApprovalNoticeParams,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalNoticeDecisionParams {
    #[serde(flatten)]
    pub notice: ApprovalNoticeParams,
    pub signature: String,
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ApprovalNoticeStatus {
    Pending,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalNoticeView {
    pub status: ApprovalNoticeStatus,
    pub approval: Option<PendingApprovalView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalPushKeyResponse {
    pub public_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalPushSubscribeParams {
    pub device_id: String,
    pub grant_id: String,
    pub generation: i32,
    pub sequence: i32,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub signature: String,
}

/// How long an approval answer lasts.
///
/// The distinction exists because "yes" is three different promises: *this time*, *until the
/// process exits*, and *until revoked*. Conflating them either nags the human on every call or
/// silently writes a standing rule they did not ask for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase", export_to = "v2/")]
pub enum RememberScope {
    /// Just this once (the default): the next identical action asks again.
    #[default]
    Once,
    /// Until this process exits: kept in memory, never written to disk.
    Session,
    /// Until revoked: written into `<home>/settings.json` (`execAllow` / `writeAllowDirs`).
    Always,
}

/// Parameters for `approval/respond`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalRespondParams {
    pub turn_id: String,
    pub request_id: String,
    pub approved: bool,
    /// How long the yes lasts: `once`, `session` (memory only), `always` (settings.json).
    ///
    /// **Required**, deliberately: no implicit default, so a client cannot forget to say how long
    /// a yes lasts and get `once` by accident. (It also keeps the generated Python model honest —
    /// an enum default comes out of `datamodel-codegen` as a bare string, a type error the SDK
    /// would then have to live with.)
    ///
    /// What gets remembered is decided by the asking tool and named on the card: for
    /// `run_command` it is the command itself (no wildcards — the human approved this line, not
    /// this kind), for `write_file`/`edit` it is the **directory** (never the file name).
    /// A remembered rule still meets hard-deny, the sandbox and the writable boundary.
    pub remember: RememberScope,
    /// The `version` of the question being answered. Absent = the client does not track
    /// versions; present and stale = refused with a text error.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub version: Option<i32>,
    /// Who answered, for the audit record: `lark:<open_id>` for a group dispatcher's card click,
    /// absent for the Runtime's owner. Recorded, never trusted for permission — the gateway
    /// already checked the clicker against the dispatcher list.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub actor: Option<String>,
}

/// One acceptance condition — what must be measurably true for the task to count as done.
///
/// The same shape is both the **input** (`task/create`) and the **view** (what is required),
/// so the card can never show a condition the verifier does not check.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct AcceptanceView {
    /// `fileExists` / `fileContains` / `jsonEquals` / `toolsOk` / `receiptConfirmed` / `commandSucceeds` / `sourceSupports` (`path` = the claims file).
    pub kind: String,
    /// Path, relative to the task's working directory (an absolute path must land inside it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Plain substring for `fileContains` — **not** a regex: a badly written pattern must not
    /// be able to stall a whole round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Occurrences required (`fileContains`) / successful calls required (`toolsOk`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<i32>,
    /// Tool name for `toolsOk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The human's own wording, when they gave one. Absent = derived from the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// RFC6901 pointer for jsonEquals; empty selects the complete document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pointer: Option<String>,
    /// JSON-encoded expected value. A string preserves the difference between absent and JSON null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_json: Option<String>,
    /// Receipt kind for `receiptConfirmed` (`feishu_send`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_kind: Option<String>,
    /// `receiptConfirmed`: a receipt whose ref starts with this, in status `confirmed`, must exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ref_prefix: Option<String>,
    /// `commandSucceeds`: the program and its arguments. Never a shell string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// `commandSucceeds`: working directory, relative to the task's directory. Absent = the root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// One condition's latest measured verdict. Same order as `AgentTaskView.acceptance`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CheckView {
    /// The condition in the human's words ("report.md contains at least 3 conclusions").
    pub label: String,
    pub passed: bool,
    /// Passed: the evidence ("report.md 12.4 KB"). Failed: the measurement ("report.md does not exist").
    pub detail: String,
    /// SHA-256 of the exact file bytes read for this measurement, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub artifact_sha256: Option<String>,
}

/// A recurring duty registered by a human. Sources identify evidence, never tool grants.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilityContract {
    pub id: String,
    pub purpose: String,
    pub sources: Vec<String>,
    pub cron: String,
    pub expires_at: String,
    #[serde(deserialize_with = "declared_budget")]
    #[schemars(schema_with = "declared_budget_schema")]
    pub budget: BudgetView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub weekly_report: Option<FeishuWeeklyReport>,
}

/// Deliver a verified research digest to this allowlisted Feishu recipient.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct FeishuWeeklyReport {
    pub task_id: String,
    pub recipient: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilityReportsParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct WeeklyReportView {
    pub wake_key: String,
    pub status: String,
    pub body: Option<String>,
    pub approval: Option<ApprovalRequestedNotification>,
    pub detail: String,
    pub tools_used: i32,
    pub wall_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilityReportsResponse {
    pub reports: Vec<WeeklyReportView>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredBudget {
    #[serde(flatten)]
    limits: BudgetView,
}

fn declared_budget<'de, D: serde::Deserializer<'de>>(input: D) -> Result<BudgetView, D::Error> {
    Ok(DeclaredBudget::deserialize(input)?.limits)
}

#[cfg(test)]
fn declared_budget_schema(
    generator: &mut schemars::gen::SchemaGenerator,
) -> schemars::schema::Schema {
    let mut schema = BudgetView::json_schema(generator).into_object();
    schema.object().additional_properties = Some(Box::new(false.into()));
    schema.into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ResponsibilityStatus {
    Active,
    Paused,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilityView {
    pub contract: ResponsibilityContract,
    pub status: ResponsibilityStatus,
    pub last_due_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilityListResponse {
    pub responsibilities: Vec<ResponsibilityView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ResponsibilitySetPausedParams {
    pub id: String,
    pub paused: bool,
}

/// A task's budget. An absent field means that dimension is uncapped.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BudgetView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<i32>,
}

/// What the task has consumed so far. Always present (zeroes when nothing ran yet).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SpentView {
    pub wall_ms: i32,
    pub tokens: i32,
    pub tools: i32,
}

/// One task as the human sees it. `status` is a string on the wire
/// (pending / running / suspended / needsAttention / done / failed / cancelled) — the dashboard paints it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct AgentTaskView {
    /// The human-selected parent coding task whose worktree receives this child's checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_task_id: Option<String>,
    /// Human-declared input obligation; it never grants tool permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub input_condition: Option<TaskInputView>,
    /// Separate task transcript, when explicitly started through task/start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub execution_session_id: Option<String>,
    /// pending / claimed / finished / interrupted; absent for conversational tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub dispatch_state: Option<String>,
    pub id: String,
    /// Absent = a task that belongs to no conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub title: String,
    /// Optional condition evaluated by the goal judge. Acceptance checks can also trigger additional rounds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_condition: Option<String>,
    pub status: String,
    pub rounds_used: i32,
    pub max_rounds: i32,
    /// The declared model tier (`fast` / `chat` / `reasoning`); absent = per-turn decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// The turn currently carrying this task (a heartbeat; absent when idle).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspend_reason: Option<String>,
    /// Why it stopped last time — the card's footnote, never guessed from the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop_reason: Option<String>,
    /// What must be measurably true. An empty list provides no independent acceptance evidence.
    #[serde(default)]
    pub acceptance: Vec<AcceptanceView>,
    /// The latest verdict per condition, same order as `acceptance`.
    #[serde(default)]
    pub checks: Vec<CheckView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetView>,
    pub spent: SpentView,
    /// The task's own working directory (the boundary for verification and writes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Set when `cwd` is a Jeeves-made git worktree: its path and branch. Never auto-deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch: Option<String>,
    /// How many times this task has been executed ("run 3 times").
    #[serde(default)]
    pub run_count: i32,
    /// Where the task was dispatched from (a chat group); absent = a local view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<InteractionSource>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

/// `task/create`: **only a human can start a long task** — there is no tool for it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskCreateParams {
    /// The human-selected parent coding task whose worktree receives this child's checkpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_task_id: Option<String>,
    /// Absent = the task belongs to no session: it can be created and inspected on its own.
    #[serde(default)]
    pub session_id: Option<String>,
    /// What this task is, in the human's words — the card's first line.
    pub title: String,
    /// Optional condition evaluated by the goal judge. Acceptance checks can also trigger additional rounds.
    #[serde(default)]
    pub goal_condition: Option<String>,
    /// Per-task round cap; absent = `JEEVES_GOAL_MAX_ROUNDS`.
    #[serde(default)]
    pub max_rounds: Option<i32>,
    /// The model tier this task runs on (`fast` / `chat` / `reasoning`). Absent = decide per
    /// turn (auto mode) or use the active profile (fixed mode). A declared tier always wins —
    /// "the task picks the model" is the whole point of the tier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// How "done" is measured. Empty = the goal/judge decides, as before.
    #[serde(default)]
    pub acceptance: Vec<AcceptanceView>,
    /// Caps on the whole task's spend. Absent dimensions are uncapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<BudgetView>,
    /// The working directory this task owns. Absent = a new managed directory under home/workspaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// A git repository (absolute path, chosen by the human). The task then runs in its own
    /// worktree on branch `jeeves/<task-id>`, which becomes its only working directory.
    /// Not combinable with `cwd`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repo: Option<String>,
    /// Test-loop checks for a coding task. `after: "final"` becomes a `commandSucceeds`
    /// acceptance condition; `after: "edit"` runs after every `edit` / `apply_patch` and its
    /// output (4 KB at most) is appended to that tool's result.
    #[serde(default)]
    pub code_checks: Vec<CodeCheckSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CodeCheckSpec {
    pub name: String,
    /// The program and its arguments. Never a shell string.
    pub argv: Vec<String>,
    /// `edit` or `final`.
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskEnvelope {
    pub task: AgentTaskView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export_to = "v2/")]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskListParams {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
}

/// Newest first; `active` is the session's newest non-terminal task, when there is one.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskListResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub next_cursor: Option<String>,
    pub tasks: Vec<AgentTaskView>,
}

/// `task/events`: every task event after `cursor` (absent = from the start), oldest first.
/// The cursor is opaque to clients: pass back `nextCursor` to continue.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskEventsParams {
    #[serde(default)]
    #[ts(optional = nullable)]
    pub cursor: Option<i32>,
    /// Narrow to one task. Absent = all tasks.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub task_id: Option<String>,
    /// At most 500; absent = 100.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub limit: Option<i32>,
}

/// One entry of a task's timeline.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskEventView {
    /// This event's own cursor.
    pub cursor: i32,
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Order within the task (1, 2, 3 …).
    pub seq: i32,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskEventsResponse {
    pub events: Vec<TaskEventView>,
    /// Resume from here. Equals the request cursor when nothing is newer.
    pub next_cursor: i32,
    pub has_more: bool,
    /// The newest cursor that exists (for the request's task filter). Lets a client tell
    /// "caught up" from "behind" without a second call.
    pub latest_cursor: i32,
}

/// `task/view`: one session's current task (the card on screen).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskViewResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<AgentTaskView>,
}

/// Start a pending task in its own durable execution context. Human-only.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskStartParams {
    pub task_id: String,
    pub message: String,
    #[serde(default)]
    #[ts(optional = nullable)]
    pub attachments: Option<Vec<TurnAttachment>>,
}

/// Human-only inspection of persisted tool operations and uncertainty.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskOperationsResponse {
    pub operations: Vec<ToolOperationView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub checkpoint_digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ToolOperationView {
    pub id: String,
    pub run_id: Option<String>,
    pub turn_id: String,
    pub round_id: String,
    pub tool_use_id: String,
    pub tool: String,
    #[ts(type = "unknown")]
    pub arguments: serde_json::Value,
    pub input_digest: String,
    /// checkpoint / recovered_checkpoint; neither proves network transmission.
    pub origin: String,
    pub status: String,
    /// Automatic replay policy; this journal grants no provider idempotency guarantees.
    pub replay_policy: String,
    pub output_digest: Option<String>,
    pub evidence: Option<String>,
    pub started_at: String,
    pub settled_at: Option<String>,
    /// Per-turn snapshot for multi-turn ordinary-chat inspection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub checkpoint_digest: Option<String>,
}

/// Evidence records a human observation and never executes or resumes the operation.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskReconcileParams {
    pub task_id: String,
    pub operation_id: String,
    pub checkpoint_digest: String,
    /// reconciled_success / reconciled_failure / not_executed.
    pub resolution: String,
    pub evidence: String,
}

/// Human-only input declaration before a task starts.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskWaitParams {
    pub task_id: String,
    pub source: String,
    pub event_key: String,
    pub description: String,
    /// Latest input arrival time; accepted data remains available after this deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Authorized host input, correlated to an existing human-declared condition.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskSignalParams {
    pub condition_id: String,
    pub source: String,
    pub event_key: String,
    pub event_id: String,
    #[ts(type = "unknown")]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskSignalResponse {
    pub task_id: String,
    pub accepted: bool,
    pub duplicate: bool,
}

/// `task/wake`: a channel message that may be the input a task is waiting for.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskWakeParams {
    /// Unique per inbound message, `<channel>:<messageId>`. A repeat is skipped, never redelivered.
    pub source_key: String,
    /// Must match the waiting condition's source (for example `lark`).
    pub source: String,
    /// Must match the waiting condition's event key (for example the chat id).
    pub event_key: String,
    #[ts(type = "unknown")]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskWakeResponse {
    /// The message was delivered to a waiting task.
    pub woken: bool,
    /// This source key was seen before; nothing was delivered again.
    pub duplicate: bool,
    /// The task that was woken (or had already been woken by this key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskInputView {
    pub condition_id: String,
    pub source: String,
    pub event_key: String,
    pub description: String,
    /// pending / received / consumed.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "unknown")]
    pub data: Option<serde_json::Value>,
}

/// `task/get`: address one task by id (a session-less task has no other way in).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskGetParams {
    pub task_id: String,
}

/// One execution of a task. A task may run several times; each attempt is its own account.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct RunView {
    pub id: String,
    /// running / waiting / suspended / completed / failed / cancelled.
    pub status: String,
    pub rounds: i32,
    pub tokens_in: i32,
    pub tokens_out: i32,
    pub tools_ok: i32,
    pub tools_failed: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

/// `task/runs`: every attempt of one task, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskRunsResponse {
    pub runs: Vec<RunView>,
}

/// One delivered file.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ArtifactView {
    /// Complete text of an observed successful whole-file write up to 64 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub written_text: Option<String>,
    pub path: String,
    pub bytes: i32,
    pub at: String,
}

/// The most recent terminal execution, including while a later revision is running.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskDeliveryView {
    pub status: String,
    pub summary: Option<String>,
    pub run_id: Option<String>,
    pub at: String,
}

/// `task/artifacts`: what this task actually produced, newest first.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskArtifactsResponse {
    pub artifacts: Vec<ArtifactView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub latest_delivery: Option<TaskDeliveryView>,
}

/// `task/suspend|resume|cancel`: address one task and say why (the reason is shown, not dropped).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskActionParams {
    pub task_id: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// A task changed state. The event bus folds it into a ping; the hub's `/api/tasks` shows the row.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskUpdatedNotification {
    pub task_id: String,
    /// Absent for a task that belongs to no session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub status: String,
    pub rounds_used: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Which session to look at — the shared params of the dashboard's per-session reads
/// (`todo/view`, `session/get`). One field, one shape, no per-method re-modeling.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionViewParams {
    pub session_id: String,
}

/// One notification per goal-loop verdict — "why did it run another round" is never guessed.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct GoalRoundNotification {
    pub turn_id: String,
    /// Which round (starting at 1).
    pub round: i32,
    pub max_rounds: i32,
    /// `continuing` / `satisfied` / `impossible` / `round-limit` / `blocked`.
    pub status: String,
    /// The judge's reason (or the reason for exiting).
    pub reason: String,
}

/// A checkpoint has been written: this turn's working memory (model thoughts, tool results) is now
/// in state.db — the loop can continue from here after a crash, finished tools are never re-run.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TurnCheckpointedNotification {
    pub turn_id: String,
    pub session_id: String,
    /// How many loop iterations are already done (a checkpoint lands at each tool-results boundary).
    pub iterations: i32,
}

/// A checkpoint is being resumed: startup reconciliation found it unfinished and the loop
/// continues from where it stood (finished tools are not re-run). The human said earlier
/// "keep going until X" by starting the turn — resuming is not the model assigning itself work.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TurnResumedNotification {
    pub turn_id: String,
    pub session_id: String,
    /// How many iterations the checkpoint had already completed.
    pub from_iterations: i32,
    /// Why it is resuming (`reconcile: crash recovery`, …).
    pub reason: String,
}

/// A checkpoint is abandoned: the human interrupted the turn (or the session moved on) —
/// it will NOT be auto-resumed. Suspended, not silently lost: the ledger says so.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TurnSuspendedNotification {
    pub turn_id: String,
    pub session_id: String,
    /// Why it was abandoned (`interrupted by user`, …).
    pub reason: String,
}

/// Response to `approval/respond`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalRespondResponse {
    /// Is this request still awaiting an answer? False when the reply arrives too
    /// late (timed out, or the turn already ended) — the client learns nobody heard its answer.
    pub accepted: bool,
    /// When `accepted` is false because someone else answered first: who (`<channel>:<id>`),
    /// so a second channel can say "already answered by …". Absent when nobody answered
    /// (expired, or the question is unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub answered_by: Option<String>,
}

/// Telemetry for one turn. Persisted into chat_log.meta, so reopening an old
/// session still shows which brain made which decision, not just the final reply.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnMeta {
    pub iterations: i32,
    pub latency_ms: i32,
    pub tools: Vec<ToolCallRecord>,
    /// The model that actually answered this turn: the small model when the graph routed quick, recorded as-is.
    pub model: String,
    pub provider: String,
    /// Token usage for this turn. The historical flaw of only reaching `turnCompleted`
    /// without being persisted is fixed: it now lands in chat_log with the meta, so past turns show their cost too.
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    /// True when this turn was cut short by `turn/interrupt`.
    /// Older metas in the DB lack this field — `#[serde(default)]` lets them parse as before.
    #[serde(default)]
    pub interrupted: bool,
    /// How many times the loop guard fired this turn (0 = the model never got stuck
    /// repeating/alternating tool calls). Zero is normal; non-zero means part of this turn's budget went to spinning in place.
    #[serde(default)]
    pub guard_hits: i32,
    /// What the guard told the model when it fired (a human-readable explanation; null otherwise).
    #[serde(default)]
    pub guard_note: Option<String>,
    /// How many provider retries happened this turn (rate limits / transient faults). Zero is the norm.
    #[serde(default)]
    pub retries: i32,
    /// The goal loop's exit (`continuing` / `satisfied` / `impossible` /
    /// `round-limit` / `blocked`). `None` when no goal is set.
    #[serde(default)]
    pub goal_status: Option<String>,
    /// How many rounds this goal has consumed up to this turn. 0 when no goal is set.
    #[serde(default)]
    pub goal_rounds: i32,
    /// How this turn's model was chosen (routing, see route.rs): `auto:reasoning → ds-r1`
    /// or `fallback: …`. `None` in fixed mode and when the chosen one is already active —
    /// the footer stays quiet and only speaks when a choice/fallback actually happened.
    #[serde(default)]
    pub route_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct Fact {
    pub id: i32,
    pub subject: String,
    pub content: String,
    /// `user` (told directly) or `consolidation` (distilled from conversations).
    pub source: String,
    /// What this fact is **about**: `user` / `feedback` / `project` / `reference` /
    /// `fact` (fallback). Anything that resists classification lands on `fact`.
    pub kind: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct Episode {
    pub id: i32,
    /// ISO 8601 date.
    pub happened_at: String,
    pub summary: String,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionSummary {
    pub id: String,
    /// The name a human gave it (migration 0018); when there is none, the first user message.
    pub title: String,
    pub messages: i32,
    pub started_at: Option<String>,
    pub last_at: Option<String>,
    /// The session's working directory: the **root of everything it may write** (tools refuse
    /// writes outside it). Absent = not chosen; the dashboard shows "choose a working directory".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cwd: Option<String>,
    /// The session this one was forked from. Display only: the two are independent — deleting
    /// one must not affect the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub forked_from: Option<String>,
}

/// A message's role. A **closed set** — deliberately no "unknown" escape hatch.
///
/// If some future `chat_log` really does grow other roles, `session/messages` will **skip**
/// those rows rather than force a value in — so the frontend `switch` can stay exhaustive
/// and "there is a third role to render" shows up at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum MessageRole {
    User,
    Assistant,
}

/// One message in session history: a row of `chat_log`.
///
/// Same conversation, second viewpoint alongside the live stream (`turnStarted` /
/// `textDelta` / `turnCompleted`): the live stream lives only for **this connection**
/// (gone on refresh); this one makes "refresh, switch devices, look back later" possible.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct Message {
    /// The chat_log row number. **Also the cursor**: it is `session/messages`' cursor.
    /// Row numbers over timestamps: both rows of one turn are written in the same second, so timestamps cannot order them.
    pub id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub source: Option<String>,
    /// Original task for a decision or completion notice in its owner's chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub task_id: Option<String>,
    /// Outcome of the run that issued this receipt, even after the task runs again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub receipt_status: Option<String>,
    pub role: MessageRole,
    pub content: String,
    /// ISO 8601 (UTC). When it was written.
    pub at: String,
    /// This turn's telemetry (gate / tools / iterations / model / usage), **present only on assistant rows**.
    pub meta: Option<TurnMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub label: Option<String>,
    pub flagship: Option<String>,
    pub fast: Option<String>,
}

/// The **read view** of config. Fields are deliberately complete and non-optional: reading it yields the full current state.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SettingsView {
    pub provider: String,
    pub model: String,
    pub small_model: String,
    pub max_iterations: i32,
    pub max_tokens: i32,
    /// Working-memory sliding window: only the last N turns go into the prompt.
    pub history_turns: i32,
    pub consolidate_every: i32,
    pub retrieval_top_k: i32,
    pub home: String,
}

/// The **write patch** for config. Pass only the fields to change; null means leave as-is.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SettingsPatch {
    #[ts(optional = nullable)]
    pub max_iterations: Option<i32>,
    #[ts(optional = nullable)]
    pub max_tokens: Option<i32>,
    #[ts(optional = nullable)]
    pub history_turns: Option<i32>,
    #[ts(optional = nullable)]
    pub consolidate_every: Option<i32>,
    #[ts(optional = nullable)]
    pub retrieval_top_k: Option<i32>,
    /// Whole-table replacement of the allowlist (`JEEVES_EXEC_ALLOW`) — what "remember this command" writes.
    #[ts(optional = nullable)]
    pub exec_allow: Option<Vec<String>>,
    /// Whole-table replacement of the directories "remember this" accepted for **writes**
    /// (`write_file`/`edit` no longer ask inside them). Remembering a directory never widens the
    /// writable boundary — the containment check still runs first.
    #[ts(optional = nullable)]
    pub write_allow_dirs: Option<Vec<String>>,
    /// The three capability switches (`JEEVES_EXEC` / `JEEVES_WRITE` / `JEEVES_DELEGATE`). A saved value
    /// overrides the environment, and takes effect on the next turn without a restart.
    #[ts(optional = nullable)]
    pub exec_enabled: Option<bool>,
    #[ts(optional = nullable)]
    pub write_enabled: Option<bool>,
    #[ts(optional = nullable)]
    pub delegate_enabled: Option<bool>,
}

impl SettingsPatch {
    /// Not a single field to change.
    ///
    /// Every field counts — an `exec_allow`-only patch that reported "empty" would make
    /// `save_patch` delete `settings.json`, i.e. throw away the very rule that was just
    /// remembered.
    pub fn is_empty(&self) -> bool {
        let p = self;
        p.max_iterations.is_none()
            && p.max_tokens.is_none()
            && p.history_turns.is_none()
            && p.consolidate_every.is_none()
            && p.retrieval_top_k.is_none()
            && p.exec_allow.is_none()
            && p.write_allow_dirs.is_none()
            && p.exec_enabled.is_none()
            && p.write_enabled.is_none()
            && p.delegate_enabled.is_none()
    }

    /// Overwrite self with the non-null fields of `newer` — for accumulating patches:
    /// the saved patch is the foundation of the new one, not the other way around.
    pub fn merge_newer(&mut self, newer: &Self) {
        let p = newer;
        if p.max_iterations.is_some() {
            self.max_iterations = p.max_iterations;
        }
        if p.max_tokens.is_some() {
            self.max_tokens = p.max_tokens;
        }
        if p.history_turns.is_some() {
            self.history_turns = p.history_turns;
        }
        if p.consolidate_every.is_some() {
            self.consolidate_every = p.consolidate_every;
        }
        if p.retrieval_top_k.is_some() {
            self.retrieval_top_k = p.retrieval_top_k;
        }
        if p.exec_allow.is_some() {
            self.exec_allow = p.exec_allow.clone();
        }
        if p.write_allow_dirs.is_some() {
            self.write_allow_dirs = p.write_allow_dirs.clone();
        }
        if p.exec_enabled.is_some() {
            self.exec_enabled = p.exec_enabled;
        }
        if p.write_enabled.is_some() {
            self.write_enabled = p.write_enabled;
        }
        if p.delegate_enabled.is_some() {
            self.delegate_enabled = p.delegate_enabled;
        }
    }
}

/// One image attached to a turn. `path` is **home-relative** (uploads land in
/// `<home>/uploads/...`); the server reads the bytes and embeds them — the model never
/// sees a path, it sees pixels. Only image files are accepted; other files keep the
/// "path mentioned in text" route.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnAttachment {
    pub path: String,
    /// Client-provided media type hint; the server trusts the file extension over this.
    #[ts(optional = nullable)]
    pub media_type: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnStartParams {
    #[ts(optional = nullable)]
    pub session_id: Option<String>,
    pub message: String,
    /// Whether to stream `textDelta` token by token. On for the dashboard, off for scripts.
    #[ts(optional = nullable)]
    pub stream: Option<bool>,
    /// Working directory for this turn (per-turn override semantics):
    /// must be an existing absolute path (fail-closed admission); relative exec commands
    /// and relative read_file/list_dir paths resolve against it. Default = process cwd (old behavior).
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    /// Images attached to this message, embedded as image content blocks. Requires a
    /// vision-capable provider; a missing or unreadable file fails the turn up front.
    #[ts(optional = nullable)]
    pub attachments: Option<Vec<TurnAttachment>>,
    /// Pin this turn to a model tier (`fast` / `chat` / `reasoning`). Absent = let the server
    /// decide (task's declared tier, auto routing, or the active profile — in that order).
    /// Machine callers (task API, webhooks) say the tier; the model itself cannot.
    #[ts(optional = nullable)]
    pub tier: Option<String>,
    /// Stable id of the message this turn answers (`[A-Za-z0-9_-]`, at most 120). A second
    /// `turn/start` with the same id in the same session replays the first turn's result
    /// instead of running again; the same id with a different message is refused.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnStartResponse {
    pub turn_id: String,
}

/// A human message in the long-lived entry point. There is no model-callable route.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct InteractionStartParams {
    pub session_id: String,
    /// Stable across retries of the same message; reuse with changed input is rejected.
    pub message_id: String,
    pub message: String,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub attachments: Option<Vec<TurnAttachment>>,
    /// Where the message came from when it is not a local view (a chat group). Absent = the
    /// owner at one of the Runtime's own views. A source whose sender is not in
    /// `JEEVES_DISPATCHERS` gets an ordinary conversation without the `task` tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source: Option<InteractionSource>,
}

/// Where a dispatched message came from, kept on every task it creates so the report can go
/// back to the same place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct InteractionSource {
    /// `lark` today.
    pub channel: String,
    pub chat_id: String,
    pub message_id: String,
    /// The sender's id on that channel (a Feishu `open_id`).
    pub sender_id: String,
}

/// The task API's one-shot shape (`POST /api/run`): a whole turn in, one answer out.
///
/// Same request body as `turn/start` (`TurnStartParams`), no stream — the HTTP call returns
/// when the turn finishes. Built for programs (CI, cron, another agent), not for a browser:
/// the dashboard keeps SSE, because a person wants to watch the answer arrive.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct RunResponse {
    pub turn_id: String,
    pub reply: String,
    /// The model that actually answered — the visible proof that a tier pin took effect.
    pub model: String,
    pub provider: String,
    /// Why this model was chosen (`tier:fast → mini`, `auto:reasoning → ds-r1`); absent = no routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnInterruptParams {
    pub turn_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnInterruptResponse {
    /// false = this turn already finished; nothing was interrupted.
    pub interrupted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionListParams {
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionListResponse {
    pub data: Vec<SessionSummary>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionNewParams {
    /// Generated by the server when absent.
    #[ts(optional = nullable)]
    pub session_id: Option<String>,
    /// The name to give it. Absent = unnamed (the list falls back to the first user message).
    #[ts(optional = nullable)]
    pub title: Option<String>,
    /// Working directory to bind it to. Must be an existing absolute path; absent = not chosen.
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionNewResponse {
    pub session_id: String,
}

/// Read what was said in a session.
///
/// **Why it exists**: without it, the conversation on the page lives only
/// in the browser — a refresh erases it. Also the prerequisite for "switch session".
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionMessagesParams {
    pub session_id: String,
    /// Fetch only messages **older** than this one (paging back through history). Absent = the newest page.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

/// Messages with the **newest first** — opposite to the conversation's direction,
/// because "paging back" is the point: take the latest page, then follow `nextCursor` into the past.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionMessagesResponse {
    /// User inputs of the current task turn, before it enters settled history. Newest page only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active_inputs: Option<Vec<String>>,
    pub data: Vec<Message>,
    /// Present only when older messages exist: pass it as the next cursor. null = the end.
    pub next_cursor: Option<String>,
}

/// Read one session's record. Same shape as a list entry — one type, no second modeling.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionGetParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionGetResponse {
    pub session: SessionSummary,
}

/// Rename a session or rebind its working directory.
///
/// **`null`/absent = leave alone; `""` = clear.** Two different intents must not collapse into
/// one: "I renamed it" and "I never touched the title" are different writes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionUpdateParams {
    pub session_id: String,
    #[ts(optional = nullable)]
    pub title: Option<String>,
    /// Must be an existing absolute path when non-empty (same admission rule as `turn/start`).
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionUpdateResponse {
    pub session: SessionSummary,
}

/// Fork a session: a new one starts from a copy of the last few exchanges.
///
/// The original keeps running untouched — this is "try another path", not "rewrite history".
/// Only the conversation is copied: tasks, checkpoints and the compaction summary stay with
/// the source (a copied summary would describe text the branch never had).
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionForkParams {
    pub session_id: String,
    /// How many exchanges (user+assistant pairs) to copy. Absent = the server's default.
    #[ts(optional = nullable)]
    pub turns: Option<u32>,
    /// The new session's id. Absent = the server picks one.
    #[ts(optional = nullable)]
    pub new_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct SessionForkResponse {
    pub session: SessionSummary,
    /// How many messages moved into the branch (0 = the source had nothing said in it yet).
    pub copied: i32,
}

/// One turn that is running **right now** (in memory — a finished turn is simply gone).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnView {
    pub turn_id: String,
    pub session_id: String,
    /// RFC 3339 with offset. The page computes "running for N minutes" from it, so the browser
    /// needs no second clock of its own.
    pub started_at: String,
}

/// Which turns are running. Absent `sessionId` = the whole home (the CLI's "what is happening").
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnListParams {
    #[ts(optional = nullable)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnListResponse {
    pub turns: Vec<TurnView>,
}

/// Add one sentence to a turn that is already running (0.16).
///
/// Not `turn/start`: that one is refused while a turn runs (`SESSION_BUSY`), and for good reason
/// — two turns on one session would interleave their writes. Steering is the supported way to
/// say "and also" without cancelling work that has already happened.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnSteerParams {
    pub session_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnSteerResponse {
    pub turn_id: String,
    /// How many additions this turn has accepted so far, this one included — "queued: 1" reads
    /// as "it will be seen", "queued: 3" as "it is piling up behind two others".
    pub queued: i32,
}

/// Queue one message to run **after** the turn that is currently running (0.16).
///
/// The difference from `turn/steer` is what the person means, not where the text goes: steer is
/// "and also, while you are at it", follow-up is "when you are done". A follow-up becomes its own
/// turn — its own rounds, budget and checkpoint — and runs even after this page is closed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnFollowUpParams {
    pub session_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnFollowUpResponse {
    pub session_id: String,
    /// Position in line: 1 = runs as soon as the current turn ends, 2 = after that one.
    pub queued: i32,
}

/// The human added a sentence to a running turn. Emitted to **every** client of the turn, so a
/// second device sees the addition at the moment it happened instead of on the next reload.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnSteeredNotification {
    pub turn_id: String,
    pub session_id: String,
    pub message: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemorySearchParams {
    pub query: String,
    #[ts(optional = nullable)]
    pub top_k: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemorySearchResponse {
    pub facts: Vec<Fact>,
    pub episodes: Vec<Episode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryRememberParams {
    pub subject: String,
    pub content: String,
    /// Defaults to `user` (said directly by the user). Consolidation passes
    /// `consolidation` when distilling from conversations, so the frontend can tell "you told it" from "it summarized itself".
    #[ts(optional = nullable)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryRememberResponse {
    /// The full fact as written, with id — the frontend can insert it directly, no re-query needed.
    pub fact: Fact,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryForgetParams {
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryForgetResponse {
    /// How many were actually deleted. 0 means there was no memory of this subject to begin with.
    pub removed: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryListParams {
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryListResponse {
    pub data: Vec<Fact>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryListEpisodesParams {
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

/// The episode list. **No cursor**: `Episodes::recent` only fetches the most recent N by
/// time desc — storage has no notion of offset. Rather than invent an unusable cursor, it honestly returns null.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct MemoryListEpisodesResponse {
    pub data: Vec<Episode>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConfigReadParams {
    /// Fetch only these keys; absent = return everything.
    #[ts(optional = nullable)]
    pub keys: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConfigReadResponse {
    pub config: SettingsView,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConfigWriteParams {
    pub patch: SettingsPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConfigWriteResponse {
    /// The full config after writing, so the frontend can replace its local cache outright.
    pub config: SettingsView,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelListParams {
    #[ts(optional = nullable)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ModelListResponse {
    pub data: Vec<ModelInfo>,
    pub next_cursor: Option<String>,
}

/// Response to `model/profiles`: all configured model profiles + the active one.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export_to = "v2/")]
pub struct ModelProfilesParams;

/// Params of `usage/summary`: an optional period, UTC, `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS`.
/// `since` is inclusive, `until` exclusive; both absent = all time.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct UsageSummaryParams {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export_to = "v2/")]
pub struct ApprovalLogParams;

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export_to = "v2/")]
pub struct ScheduleListParams;

/// One scheduled job as the capability center shows it: the declaration plus the runtime state.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ScheduleJob {
    pub name: String,
    /// Five-field cron, exactly as declared.
    pub cron: String,
    pub prompt: String,
    /// outbox / notify / quiet.
    pub delivery: String,
    /// true = at-least-once (re-run once after a miss), false = at-most-once (give up).
    pub catch_up: bool,
    /// `store` (saved in state.db, editable) or `skill` (a SKILL.md `schedule:` line, read-only here).
    pub source: String,
    /// The skill this job comes from, when `source == "skill"`.
    pub skill: Option<String>,
    /// The last settled trigger minute (the dedup anchor); null if it never ran.
    pub last_fired_minute: Option<String>,
    /// Non-null = the last run was interrupted; it is never replayed, a human acknowledges it.
    pub claimed_minute: Option<String>,
    /// The next fire, or null when the pattern can never match.
    pub next_run: Option<String>,
    /// Why `next_run` is null, when it is null for an interesting reason.
    pub next_run_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ScheduleListResponse {
    /// All jobs sorted by name; an empty array when none was ever declared.
    pub jobs: Vec<ScheduleJob>,
    /// Why part of the answer is missing (a state read failed), so "never ran" and "cannot tell" differ.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ScheduleSaveParams {
    pub name: String,
    pub cron: String,
    pub prompt: String,
    /// Empty or absent = outbox.
    #[ts(optional = nullable)]
    pub delivery: Option<String>,
    /// Absent = true.
    #[ts(optional = nullable)]
    pub catch_up: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ScheduleRemoveParams {
    pub name: String,
}

/// What `schedule/save` and `schedule/remove` answer: one sentence for the panel or the model.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ScheduleChangeResponse {
    pub message: String,
}

/// A profile's **view**: unlike the stored copy it contains no secret, only where the secret lives.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelProfileView {
    pub id: String,
    /// The provider id in `jeeves-provider` terms.
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Where the credential lives: `keychain` / `file` / `env:VAR` / `memory` / none (local model).
    /// **A pointer, not the key itself** — the UI uses it to honestly show "where your key is stored".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_source: Option<String>,
    /// Whether this profile's model accepts images.
    /// `null` = follow the vendor's default model (the catalog row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub capabilities: Option<ModelCapabilities>,
    /// Whether this profile is currently active.
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelProfilesResponse {
    /// The currently active id (null when nothing is configured).
    pub active_id: Option<String>,
    pub profiles: Vec<ModelProfileView>,
}

/// Parameters for `model/save`. `provider` is required only when **creating** —
/// passing only `id` + `make_active` means switch.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelSaveParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub small_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The key itself. Absent = keep as-is (changing models or switching never requires re-sending).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Use a key from an environment variable (pass a name like `DEEPSEEK_API_KEY`):
    /// Jeeves only reads it and never copies it into any file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_env: Option<String>,
    /// Whether this model accepts images. Absent = keep the vendor's default (patch
    /// semantics, same as `model`); `true` is how "this vendor's default is text-only,
    /// but *this* model sees" gets declared at the moment the profile is created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vision: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub capabilities: Option<ModelCapabilities>,
    /// Make it active right after saving (default true — the smoothest on first setup).
    #[serde(default)]
    pub make_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelSaveResponse {
    pub id: String,
    /// Whether this profile is currently active.
    pub active: bool,
    pub active_id: Option<String>,
    /// Where the credential **actually** landed: `keychain` / `file` / `env:VAR` / `memory` / `none`.
    /// Fall back to a plaintext file and say so honestly — never let the user assume the keychain.
    pub key_stored_in: String,
    /// The current model after switching (`provider / model`); null when not fully configured.
    pub current: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelRemoveParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelRemoveResponse {
    pub removed: bool,
    /// When the active profile is removed, active is cleared along with it.
    pub active_id: Option<String>,
}

/// One tier's entry in the routing table. `profile_id` absent = "follow the fallback chain"
/// (`reasoning` → `chat` → active, `fast` → `chat` → active), i.e. what the router does anyway.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TierRoute {
    /// fast / chat / reasoning.
    pub tier: String,
    /// `null` is a real answer ("follow the chain"), so it is always on the wire — the panel
    /// distinguishes "not configured" from "field missing" by reading this, not by guessing.
    #[serde(default)]
    pub profile_id: Option<String>,
}

/// `model/routes`: the routing table as the panel shows it. Always three rows (the tiers are
/// fixed), each maybe pointing at nothing — an empty table is a valid, honest state.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelRoutesResponse {
    /// `fixed` (default, active profile wins) or `auto` (per-turn tier pick).
    pub mode: String,
    pub routes: Vec<TierRoute>,
}

/// `model/routes/save`: write the whole table (no patch semantics — three rows are cheap and
/// "replace the table" cannot leave a half-edited state behind).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelRoutesSaveParams {
    pub mode: String,
    #[serde(default)]
    pub routes: Vec<TierRoute>,
}

/// Response to `usage/summary`: per-model aggregates + totals. In hub mode this is per-user
/// bookkeeping (one home and one state.db per user — the ledger is isolated by person by construction).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct UsageByModel {
    pub provider: String,
    pub model: String,
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct UsageSummaryResponse {
    pub turns: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub by_model: Vec<UsageByModel>,
}

/// One row of `approval/log`: the complete record of one "ask before running".
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalLogEntry {
    pub turn_id: String,
    pub request_id: String,
    pub tool: String,
    pub args_preview: String,
    pub reason: String,
    /// One of approved / denied / timeout / voided.
    pub outcome: String,
    /// Who answered: `owner`, or the channel identity that clicked (`lark:<open_id>`). Empty for
    /// a timeout or a voided request, and for rows written before this was recorded.
    pub actor: String,
    pub decided_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ApprovalLogResponse {
    /// Newest first, capped at 100 entries by the server.
    pub entries: Vec<ApprovalLogEntry>,
}

/// Everything the dashboard's first screen needs, fetched in one shot.
///
/// It is not the response to any RPC but the payload of the `jeeves dashboard` HTTP
/// endpoint (`GET /api/data`). **Why it lives in the protocol crate**: the only
/// criterion is that it crosses languages. The browser side type-checks against it, and hand-copying
/// it would drift again — exactly what this whole effort exists to eliminate.
///
/// Fields reuse protocol types directly: the dashboard does no second modeling —
/// it is just a view over these data.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct DashboardData {
    /// ISO 8601, millisecond precision. The page shows "when is this data from".
    pub generated_at: String,
    pub config: SettingsView,
    pub sessions: Vec<SessionSummary>,
    pub facts: Vec<Fact>,
    pub episodes: Vec<Episode>,
    /// The viewed session's todo sheet, when asked for and non-empty. This is the
    /// model's own working sheet (todo_write), shown to the human as the task card.
    #[serde(default)]
    #[ts(optional)]
    pub todos: Option<TodoListView>,
    /// The viewed session's current task (goal upgraded to a persisted entity), when asked for.
    #[serde(default)]
    #[ts(optional)]
    pub task: Option<AgentTaskView>,
    /// What that task actually produced: files that exist, with their sizes (0.14). Always
    /// present (empty when there is no task), so the card needs no branch to render it.
    #[serde(default)]
    pub task_artifacts: Vec<ArtifactView>,
    /// Connectors configured on this machine (0.12.0). Read from connectors.json by ops:
    /// it is the same file the app-server registered tools from, on the same machine.
    #[serde(default)]
    pub connectors: Vec<ConnectorStatus>,
    /// Why the connector file could not be read, if it could not — the panel shows this
    /// instead of silently pretending there are no connectors.
    #[serde(default)]
    #[ts(optional)]
    pub connectors_error: Option<String>,
}

/// One configured connector, as far as the dashboard needs to know: which one, whether
/// it can actually send, and how wide the door is (allowlist size).
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConnectorStatus {
    /// `email` / `feishu`.
    pub id: String,
    /// `false` = draft-only: nothing leaves the machine.
    pub live: bool,
    /// How many entries the recipient allowlist has. `0` means nobody — said, not implied.
    pub allow_recipients: usize,
}

/// One row of the todo sheet. `status` is the wire form of `TodoStatus`
/// (pending / in_progress / completed / cancelled) — a string, not an enum:
/// the sheet is the model's vocabulary, the dashboard only paints it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TodoItemView {
    pub content: String,
    pub status: String,
}

/// The whole sheet as the human sees it. `revision` is the board's monotonic
/// version — the UI can tell "changed" from "refetched" with it.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TodoListView {
    pub revision: i64,
    pub items: Vec<TodoItemView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnStartedNotification {
    pub turn_id: String,
    pub session_id: String,
    pub user_message: String,
    /// ISO 8601, millisecond precision.
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TextDeltaNotification {
    pub turn_id: String,
    /// A streaming text delta. Present only when `turn/start` passed `stream: true`.
    pub delta: String,
}

/// A "retrying now" notice.
///
/// Retries are **never silent**: the user sees this message, not an inexplicable long pause.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct RetryNotification {
    pub turn_id: String,
    /// Which retry attempt (starting at 1).
    pub attempt: i32,
    /// Why it is retrying (`HTTP 429` / `network error: …`).
    pub reason: String,
    /// How long it waited.
    pub delay_ms: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ToolStartedNotification {
    pub turn_id: String,
    /// The id the model minted for this call — the same id its result comes back under.
    /// Clients key on it: several read-only calls can be in flight at once.
    pub tool_use_id: String,
    pub tool: String,
    /// The model-supplied arguments; structure depends on the tool.
    #[ts(type = "unknown")]
    pub args: serde_json::Value,
}

/// Live output from a tool that is **still running** (a long command's stdout).
///
/// Progress only: the `toolCompleted` that follows carries the real result, and that is what
/// the model reads. A client that ignores this event loses liveness, not correctness.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ToolDeltaNotification {
    pub turn_id: String,
    pub tool_use_id: String,
    /// A chunk of output, as it arrived. Chunk boundaries are not meaningful.
    pub chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ToolCompletedNotification {
    pub turn_id: String,
    /// Same id as the matching `toolStarted`, so a client can settle the right row.
    pub tool_use_id: String,
    pub tool: String,
    /// The text the tool returned to the model. Failure details live here too — the turn is not aborted.
    pub output: String,
    pub status: ToolStatus,
    pub duration_ms: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ConsolidationCompletedNotification {
    /// How many new facts this batch distilled; 0 means not due yet or nothing surfaced.
    pub new_facts: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct TurnCompletedNotification {
    pub turn_id: String,
    pub reply: String,
    pub iterations: i32,
    pub meta: TurnMeta,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ErrorNotification {
    pub code: i32,
    pub message: String,
    #[ts(type = "unknown")]
    pub data: Option<serde_json::Value>,
}

/// Human-selected policy, always bounded by enabled tools and their hard constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum PermissionMode {
    ReadOnly,
    Standard,
    FullAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SessionPermissionGetParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SessionPermissionSetParams {
    pub session_id: String,
    pub mode: PermissionMode,
    pub expected_revision: i32,
    /// Explicit human acknowledgement is mandatory when entering full access.
    pub confirm_full_access: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SessionPermissionResponse {
    pub session_id: String,
    /// Null preserves pre-existing runtime rules until a human selects a preset.
    pub mode: Option<PermissionMode>,
    pub revision: i32,
    pub exec_enabled: bool,
    pub write_enabled: bool,
    pub delegate_enabled: bool,
}

/// The full stream of events for one turn.
///
/// `type` is the discriminant — the frontend switches on it, Python uses a pydantic discriminated
/// union. **No failure here should crash the client**: this only describes "what happened".
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type")]
#[ts(export_to = "v2/")]
pub enum ServerNotification {
    PermissionChanged(SessionPermissionResponse),
    TurnStarted(TurnStartedNotification),
    TextDelta(TextDeltaNotification),
    /// Retrying now (rate limit / transient fault): **sent every time**, never silent.
    Retry(RetryNotification),
    /// A command is waiting for approval: answer it first, then the turn continues.
    ApprovalRequested(ApprovalRequestedNotification),
    GoalRound(GoalRoundNotification),
    ToolStarted(ToolStartedNotification),
    /// A chunk of live output from a running tool. Progress, not a result.
    ToolDelta(ToolDeltaNotification),
    ToolCompleted(ToolCompletedNotification),
    ConsolidationCompleted(ConsolidationCompletedNotification),
    /// A checkpoint is being resumed after crash recovery (finished tools are not re-run).
    TurnCheckpointed(TurnCheckpointedNotification),
    /// Someone added a sentence to a turn that was already running (0.16).
    TurnSteered(TurnSteeredNotification),
    TurnResumed(TurnResumedNotification),
    TurnSuspended(TurnSuspendedNotification),
    /// The task behind this turn changed state (created / running / suspended / done…).
    TaskUpdated(TaskUpdatedNotification),
    /// Boxed: its `TurnMeta` is an order of magnitude bigger than the others, and
    /// notifications are built per event and shipped via channels. ts-rs is transparent to `Box<T>`.
    TurnCompleted(Box<TurnCompletedNotification>),
    Error(ErrorNotification),
}

/// Human-only replacement of a fact's validity window. Null removes a bound.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MemoryValidityParams {
    pub fact_id: i32,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MemoryProvenanceParams {
    pub fact_id: i32,
}

/// References describe extraction context, not independent evidence of truth.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MemoryObservation {
    pub context_digest: String,
    pub source_refs: Vec<String>,
    pub observed_at: String,
    /// JSON assessment for a source-bound temporary claim. Confidence is model-reported,
    /// not calibrated or independent verification; includes quotation and source-time expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub assessment_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MemoryProvenanceResponse {
    pub fact_id: i32,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    /// `human` or `consolidation`; absent for an unbounded legacy claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub validity_origin: Option<String>,
    pub observations: Vec<MemoryObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MemoryValidityResponse {
    pub changed: bool,
}

/// Explicit declarations for the profile's main model; absent fields retain defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ModelCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub context_window: Option<i32>,
    /// Responses only. Supported efforts depend on the selected model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub reasoning_effort: Option<String>,
}

/// Human-only inspection of stopped ordinary-chat operations; task operations use task/operations.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TurnOperationsParams {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub turn_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TurnReconcileParams {
    pub session_id: String,
    pub turn_id: String,
    pub operation_id: String,
    pub checkpoint_digest: String,
    pub resolution: String,
    pub evidence: String,
}

/// One shell's declaration at connection time. `platform` / `transport` are labels for
/// diagnostics, not a permission boundary: the engine never branches on them.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SessionCapabilitiesParams {
    /// e.g. "web", "macos", "windows", "android", "harmony".
    pub platform: String,
    /// How this shell reaches the engine: "http-sse", "tauri-ipc", "ffi", "stdio".
    pub transport: String,
    #[ts(optional = nullable)]
    pub client_version: Option<String>,
    /// Capability names this shell can render or supply, e.g. "streaming", "images",
    /// "approval", "attachments". Unknown names are refused, not silently ignored.
    pub features: Vec<String>,
}

/// The engine's answer. `accepted` / `unsupported` partition `features`; a newer client
/// talking to an older engine sees the gap instead of guessing.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct SessionCapabilitiesResponse {
    /// Bumped only on a breaking change to the wire contract.
    pub protocol_version: i32,
    pub accepted: Vec<String>,
    pub unsupported: Vec<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum DeviceCapability {
    BrowserControl,
    DesktopControl,
    ApprovalReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DevicePairParams {
    pub device_id: String,
    pub expected_generation: i32,
    pub capabilities: Vec<DeviceCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DeviceRevokeParams {
    pub device_id: String,
    pub expected_generation: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DeviceGrantView {
    pub device_id: String,
    pub generation: i32,
    pub grant_id: String,
    pub capabilities: Vec<DeviceCapability>,
    pub revoked: bool,
    pub last_sequence: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceGrantView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DeviceReportParams {
    pub device_id: String,
    pub grant_id: String,
    pub generation: i32,
    pub capability: DeviceCapability,
    pub sequence: i32,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DeviceReportResponse {
    pub sequence: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ExecutorControlView {
    pub stopped: bool,
    pub generation: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ExecutorReleaseParams {
    pub expected_generation: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserHumanKey {
    Enter,
    Tab,
    Backspace,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserScrollDirection {
    Up,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[ts(tag = "kind", rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserHumanInput {
    Click { x: i32, y: i32 },
    Text { text: String },
    Key { key: BrowserHumanKey },
    Scroll { direction: BrowserScrollDirection },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserHumanInputParams {
    pub task_id: String,
    pub generation: i32,
    pub frame_id: String,
    pub input: BrowserHumanInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserDownloadStatus {
    Receiving,
    Pending,
    Saved,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserDownloadAction {
    Save,
    Cancel,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserDownloadParams {
    pub task_id: String,
    pub generation: i32,
    pub download_id: String,
    pub action: BrowserDownloadAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserDownloadView {
    pub id: String,
    pub name: String,
    pub status: BrowserDownloadStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserDownloadsResponse {
    pub downloads: Vec<BrowserDownloadView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum DesktopAction {
    Click,
    ScrollUp,
    ScrollDown,
    SetText,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopElement {
    pub id: i32,
    pub label: String,
    pub actions: Vec<DesktopAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopObservation {
    pub observation_id: String,
    pub window: String,
    pub elements: Vec<DesktopElement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopActParams {
    pub task_id: String,
    pub observation_id: String,
    pub element_id: i32,
    pub action: DesktopAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional=nullable)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopReceipt {
    pub id: String,
    pub status: String,
    pub action: DesktopAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopStatus {
    pub available: bool,
    pub selected: bool,
    pub screen_profile: Option<String>,
    pub receipts: Vec<DesktopReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopModelParams {
    pub task_id: String,
    pub profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopFrameParams {
    pub task_id: String,
    pub observation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopFrame {
    pub frame_id: String,
    pub data_url: String,
    pub destination: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopShareParams {
    pub task_id: String,
    pub frame_id: String,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct DesktopAdvice {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum DesktopWorkerCommand {
    Select,
    Observe,
    Act {
        observation_id: String,
        element_id: i32,
        action: DesktopAction,
        text: Option<String>,
    },
    Frame {
        observation_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum DesktopWorkerReply {
    Observed { observation: DesktopObservation },
    Frame { jpeg: String },
    Denied { message: String },
    Reselect { message: String },
    Unknown { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserListParams {
    pub session_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserListResponse {
    pub available: bool,
    pub sessions: Vec<BrowserSessionView>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserStartParams {
    pub task_id: String,
    pub url: String,
    #[serde(default)]
    pub origins: Vec<String>,
    #[serde(default)]
    pub allow_localhost: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserControlParams {
    pub task_id: String,
    pub generation: i32,
    pub action: BrowserControlAction,
    pub origin: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserControlAction {
    Takeover,
    Return,
    Pause,
    Close,
    AllowOrigin,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserSessionView {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub generation: i32,
    pub origins: Vec<String>,
    pub url: String,
    pub title: String,
    pub observed_at: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserFrameResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub frame_id: Option<String>,
    pub data_url: String,
    pub observed_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserElement {
    pub reference: String,
    pub role: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserObservation {
    pub id: String,
    pub url: String,
    pub title: String,
    pub document_version: i32,
    pub elements: Vec<BrowserElement>,
    pub text: String,
    pub collected_at: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserInputKind {
    Navigate,
    Click,
    Fill,
    Scroll,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserActInput {
    pub expected_name: Option<String>,
    pub kind: BrowserInputKind,
    pub observation_id: String,
    pub reference: Option<String>,
    pub url: Option<String>,
    pub text: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum BrowserWorkerCommand {
    Hello,
    Launch {
        url: String,
        origins: Vec<String>,
        allow_localhost: bool,
        headless: bool,
        generation: i32,
    },
    Observe,
    Act {
        generation: i32,
        expires_at: String,
        input: BrowserActInput,
    },
    Control {
        generation: i32,
        action: BrowserControlAction,
        origin: Option<String>,
    },
    HumanInput {
        generation: i32,
        frame_id: String,
        input: BrowserHumanInput,
    },
    Downloads,
    Download {
        generation: i32,
        download_id: String,
        action: BrowserDownloadAction,
    },
    Frame,
    Heartbeat,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserWorkerRequest {
    pub id: i32,
    pub command: BrowserWorkerCommand,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserWorkerState {
    pub generation: i32,
    pub status: String,
    pub observation: Option<BrowserObservation>,
    pub blocked_origins: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserWorkerResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub downloads: Option<Vec<BrowserDownloadView>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub download_data: Option<String>,
    pub info: BrowserWorkerInfo,
    pub id: i32,
    pub state: Option<BrowserWorkerState>,
    pub frame: Option<BrowserFrameResponse>,
    pub error: Option<String>,
    pub uncertain: bool,
}

/// Every worker response carries a compatible handshake and process identity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct BrowserWorkerInfo {
    pub schema_version: i32,
    pub instance_id: String,
    pub capabilities: Vec<String>,
    pub max_origins: i32,
    pub max_frame_bytes: i32,
}

/// What kind of thing a capability card describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum CapabilityKind {
    Channel,
    Connector,
    Mcp,
    Model,
    Executor,
}

/// Whether a capability can be used right now, and if not, why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum CapabilityState {
    Ready,
    Unauthorized,
    Expired,
    MissingScope,
    BlockedByPolicy,
    Offline,
    Unknown,
}

/// Conservative effect used by both local tools and owner-declared connector manifests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ToolEffect {
    Read,
    Draft,
    #[default]
    Write,
    Send,
    Delete,
}

impl ToolEffect {
    pub fn is_read(self) -> bool {
        self == Self::Read
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ConnectorCategory {
    Calendar,
    Mail,
    Tasks,
    Notes,
    Files,
    Contacts,
    Maps,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorToolRule {
    #[serde(default)]
    pub effect: ToolEffect,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub resource: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorManifest {
    pub category: ConnectorCategory,
    #[serde(default)]
    pub tools: std::collections::BTreeMap<String, ConnectorToolRule>,
    #[serde(default)]
    pub verbs: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub enum ConnectorSetup {
    Builtin,
    None,
    Bearer,
    Oauth,
    ProviderSetup,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorCatalogEntry {
    pub id: String,
    pub title: String,
    pub documentation_url: String,
    pub setup: ConnectorSetup,
    pub endpoint: Option<String>,
    pub manifest: ConnectorManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorCatalogResponse {
    pub entries: Vec<ConnectorCatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorToolsSetParams {
    pub id: String,
    pub tool: String,
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorToolView {
    pub name: String,
    pub effect: ToolEffect,
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorToolsResponse {
    pub editable: bool,
    pub tools: Vec<ConnectorToolView>,
}

/// One capability and its state. Never carries a secret: `secretPointer` says where a key
/// lives (`env:NAME` / `keychain` / `file`), not what it is.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CapabilityCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub manifest: Option<ConnectorManifest>,
    /// Stable id such as `model:ds`, `mcp:github`, `connector:feishu`.
    pub id: String,
    pub kind: CapabilityKind,
    pub title: String,
    /// Who it acts as (an app id, a profile's provider), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Which tools this capability supplies (`mcp:<server>`, `feishu_send`, `model`).
    pub tool_source: String,
    pub state: CapabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_pointer: Option<String>,
    /// Unix seconds of the last probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, JsonSchema)]
#[ts(export_to = "v2/")]
pub struct CapabilityListParams;

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CapabilityListResponse {
    pub cards: Vec<CapabilityCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CapabilityProbeParams {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CapabilityProbeResponse {
    pub card: CapabilityCard,
}

/// Human-only configuration. These fields contain references to credentials, never their values.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct CalendarConnectorSettings {
    pub url: String,
    pub username: Option<String>,
    pub credential_source: Option<String>,
    pub timezone: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase", export_to = "v2/")]
pub enum MailConnectorSecurity {
    Tls,
    Starttls,
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct MailConnectorServer {
    pub host: String,
    pub port: i32,
    pub security: MailConnectorSecurity,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct EmailConnectorSettings {
    pub username: String,
    pub credential_source: String,
    pub from: String,
    pub imap: MailConnectorServer,
    pub smtp: MailConnectorServer,
    pub allow_recipients: Vec<String>,
    pub live: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorSettingsResponse {
    pub calendar: Option<CalendarConnectorSettings>,
    pub email: Option<EmailConnectorSettings>,
}

/// Exactly one section is provided; the other and Feishu configuration stay unchanged.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorSettingsSaveParams {
    #[serde(default)]
    pub calendar: Option<CalendarConnectorSettings>,
    #[serde(default)]
    pub email: Option<EmailConnectorSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorOAuthStartParams {
    pub id: String,
}

/// A prepared authorization URL. Its loopback callback is on the Runtime host.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct ConnectorOAuthStartResponse {
    pub authorization_url: String,
}

/// Integrate the exact reviewed head of a completed child into its pending parent worktree.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskIntegrateParams {
    pub parent_task_id: String,
    pub child_task_id: String,
    pub commit: String,
    pub confirmed: bool,
}

/// Explicit human approval of the exact relative paths to commit in the task worktree.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskGitCheckpointParams {
    pub task_id: String,
    pub paths: Vec<String>,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskGitCheckpointResponse {
    pub task_id: String,
    pub commit: String,
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskGitCheckpointPolicyParams {
    pub task_id: String,
    pub paths: Vec<String>,
    pub enabled: bool,
    pub confirmed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "v2/")]
pub struct TaskGitCheckpointPolicyResponse {
    pub task_id: String,
    pub enabled: bool,
    pub paths: Vec<String>,
}
