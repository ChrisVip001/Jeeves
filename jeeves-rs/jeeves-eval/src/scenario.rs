//! The shape of a deterministic case. One case = scripted model replies + assertions on harness behavior.
//!
//! Single-turn cases write `message` / `responses` directly; cross-turn behavior
//! (sliding window, folding, consolidation, config reload, session isolation) uses `turns`.

use jeeves_provider::mock::Mock;
use jeeves_provider::CreateResponse;
use serde::Deserialize;
use serde_json::Value;

use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub id: String,
    #[serde(default)]
    pub description: String,
    /// Single-turn convenience fields: either these or `turns`, not both.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub responses: Vec<ScriptedResponse>,
    /// Single-turn convenience field: interrupt after the Nth streaming increment.
    #[serde(default)]
    pub interrupt_after_deltas: Option<usize>,
    /// This turn's chosen workspace — `{{home}}` expands to the case's temp home. Write
    /// tools have no writable root unless a workspace was chosen, so write cases must set it.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Multi-turn: one script per turn (responses / session id / interrupt / config patch).
    #[serde(default)]
    pub turns: Option<Vec<TurnScript>>,
    #[serde(default)]
    pub expect: Expectations,
    /// Overrides a few Settings knobs (sliding window, consolidate frequency, …).
    #[serde(default)]
    pub settings: SettingsOverrides,
    /// Files written into home before the run (relative paths) — for mcp.json and fake servers.
    #[serde(default)]
    pub files: Option<std::collections::BTreeMap<String, String>>,
    /// Skip this case when a prerequisite is missing ("python3" / "sh" / "sandbox"). A skip is not a failure.
    #[serde(default)]
    pub prereq: Option<String>,
    /// A behavior Jeeves does not have yet, named by the plan step that will add it. The case runs
    /// and its reasons are printed, but a failure does not block the release; a **pass** does,
    /// so the marker is removed the moment the gap closes and cannot go stale.
    #[serde(default)]
    pub known_gap: Option<String>,
    /// A long task created **before** the first turn (0.14): the turn then carries it, and the
    /// assertions read the real row back. This is what makes "the model claims done, the file is
    /// not there" a deterministic, offline case.
    #[serde(default)]
    pub task: Option<ScenarioTask>,
    /// This case measures **interrupt recovery** (P1 long benchmarks): a turn is cut short, the
    /// server restarts, and a later turn must finish the task. Only the long suite reads this
    /// flag — it splits "did the task succeed" from "did it recover from an interruption".
    #[serde(default)]
    pub recovery: bool,
    /// The planner's scripted reply (`JEEVES_PLAN`). The plan call happens **before** the first
    /// round, so it sits before that round's first reply in the mock queue — this field is the only
    /// way to say that in call order.
    #[serde(default)]
    pub plan: Option<String>,
}

/// The task a case creates up front. Mirrors `task/create`'s params (minus the noise).
#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioTask {
    pub title: String,
    #[serde(default)]
    pub goal_condition: Option<String>,
    /// Which session carries it; unset uses the main session.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Each condition is `{"kind":"fileExists","path":"report.md"}` etc. — the wire shape.
    #[serde(default)]
    pub acceptance: Vec<jeeves_protocol::AcceptanceView>,
    #[serde(default)]
    pub budget: Option<jeeves_protocol::BudgetView>,
    /// Round cap; unset = the settings' `goal_max_rounds`.
    #[serde(default)]
    pub max_rounds: Option<i32>,
    /// Receipts to seed for the task before its turns run (`claimed` / `confirmed` / `failed`).
    #[serde(default)]
    pub receipts: Vec<ScenarioReceipt>,
    #[serde(default)]
    pub code_checks: Vec<jeeves_protocol::CodeCheckSpec>,
}

/// One seeded receipt: what a tool claimed, and whether a read-back confirmed it.
#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioReceipt {
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub status: String,
}

/// How the harness answers one approval question (`JEEVES_APPROVAL=on-request`).
///
/// `silence` is the interesting one: nobody answers, and the case proves the **server's** timeout
/// turns that into a refusal instead of a hung conversation. That is a property of the approval
/// bridge, not of the tool, so it can only be measured end to end.
#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalScript {
    /// `yes` / `no` / `silence`.
    pub answer: String,
    /// `once` (default) / `session` / `always` — how long the yes lasts.
    #[serde(default)]
    pub remember: Option<String>,
}

/// The script for one turn of a multi-turn case.
#[derive(Debug, Clone, Deserialize)]
pub struct TurnScript {
    pub message: String,
    /// Session label for standalone turns. Unset uses the isolated "eval" session.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub responses: Vec<ScriptedResponse>,
    /// A `config/write` patch applied after this turn (verifies hot config reload).
    #[serde(default)]
    pub apply_patch: Option<Value>,
    /// Interrupt this turn after the Nth streaming increment (verifies turn/interrupt).
    #[serde(default)]
    pub interrupt_after_deltas: Option<usize>,
    /// The workspace this turn picked (see `Scenario::cwd`).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Reopen a fresh runtime on the same home; actual process death is tested by smoke-recovery.
    #[serde(default)]
    pub restart: bool,
    /// Explicit human task/resume action; message is the control reason, not a new chat turn.
    #[serde(default)]
    pub resume_task: bool,
    /// How to answer approval questions during this turn. Unset = nobody answers, which with
    /// `approval` on is a refusal after the timeout (the default the tool layer promises).
    #[serde(default)]
    pub approval: Option<ApprovalScript>,
}

/// Case-level overrides of Settings knobs.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsOverrides {
    #[serde(default)]
    pub history_turns: Option<i32>,
    #[serde(default)]
    pub consolidate_every: Option<i32>,
    #[serde(default)]
    pub retrieval_top_k: Option<i32>,
    #[serde(default)]
    pub max_iterations: Option<i32>,
    #[serde(default)]
    pub max_tokens: Option<i32>,
    /// Approximate context window override — token-triggered compaction must be pinnable in a case.
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub compact_threshold: Option<f64>,
    /// Exec tool toggle and allow rules (see jeeves-tools' exec.rs).
    #[serde(default)]
    pub exec_enabled: Option<bool>,
    /// File-write tools (`JEEVES_WRITE`).
    #[serde(default)]
    pub write_enabled: Option<bool>,
    /// Subagents (`JEEVES_DELEGATE`).
    #[serde(default)]
    pub delegate_enabled: Option<bool>,
    /// The P1 planner (`JEEVES_PLAN`): one plan call before the first round.
    #[serde(default)]
    pub plan_enabled: Option<bool>,
    #[serde(default)]
    pub exec_allow: Option<Vec<String>>,
    /// In-turn tool result budget (see jeeves-loop's budget.rs).
    #[serde(default)]
    pub tool_result_total_chars: Option<i64>,
    #[serde(default)]
    pub tool_result_max_chars: Option<i64>,
    /// Prompt shape: how many of the newest tool results are never stubbed.
    #[serde(default)]
    pub tool_result_keep_recent: Option<i32>,
    /// `auto` / `thin` / `full` (`JEEVES_SOUL_MODE`).
    #[serde(default)]
    pub soul_mode: Option<String>,
    /// `eager` / `deferred` (`JEEVES_TOOL_POLICY`).
    #[serde(default)]
    pub tool_policy: Option<String>,
    /// Skill catalog budget and one body's cap.
    #[serde(default)]
    pub skill_index_tokens: Option<i32>,
    #[serde(default)]
    pub skill_body_tokens: Option<i32>,
    /// The meter's warning threshold (`JEEVES_CONTEXT_WARN_RATIO`).
    #[serde(default)]
    pub context_warn_ratio: Option<i32>,
    /// `never` / `on-request` (`JEEVES_APPROVAL`) — the latter makes writes and commands ask a human.
    #[serde(default)]
    pub approval: Option<String>,
    /// `JEEVES_APPROVAL_TIMEOUT`: shortened so a "nobody answers" case costs a second, not two minutes.
    #[serde(default)]
    pub approval_timeout_secs: Option<i64>,
    /// `JEEVES_AUTO_CONTINUE_MAX`: how many times a turn may continue itself. Cases that end on
    /// purpose (an unfinished todo is part of the assertion) set this to 0.
    #[serde(default)]
    pub auto_continue_max: Option<i32>,
    /// `JEEVES_TURN_STEP_SLICE`: steps granted by one continuation — small in a case, so "the budget
    /// ran out and it kept going" is cheap to prove.
    #[serde(default)]
    pub turn_step_slice: Option<i32>,
}

impl Scenario {
    /// Expands to per-turn scripts: `turns` wins; otherwise one turn is synthesized from the single-turn fields.
    pub fn effective_turns(&self) -> Vec<TurnScript> {
        if let Some(turns) = &self.turns {
            return turns.clone();
        }
        vec![TurnScript {
            message: self.message.clone().unwrap_or_default(),
            session_id: None,
            responses: self.responses.clone(),
            apply_patch: None,
            interrupt_after_deltas: self.interrupt_after_deltas,
            cwd: self.cwd.clone(),
            restart: false,
            resume_task: false,
            approval: None,
        }]
    }

    /// The mock reply queue = all turn scripts flattened in execution order. Extra model
    /// calls like consolidation are inserted in time order, so case authors just follow
    /// run_turn's real order. Uniform streaming: interrupt cases depend on increments; others
    /// don't (assertions read the assembled full text).
    pub fn build_mock(&self, home: &std::path::Path) -> Mock {
        let mut responses: Vec<CreateResponse> = Vec::new();
        if let Some(plan) = &self.plan {
            responses.push(Mock::text(plan));
        }
        for turn in self.effective_turns() {
            for (index, response) in turn.responses.iter().enumerate() {
                responses.push(response.to_create_response(index, home));
            }
        }
        Mock::streaming(responses)
    }
}

/// The model's next move: plain text or one tool call, listed in call order.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptedResponse {
    Text(String),
    ToolUse { name: String, args: Value },
}

impl ScriptedResponse {
    pub(crate) fn to_create_response(
        &self,
        index: usize,
        home: &std::path::Path,
    ) -> CreateResponse {
        match self {
            ScriptedResponse::Text(text) => Mock::text(text),
            ScriptedResponse::ToolUse { name, args } => {
                let replaced = crate::expand_home(&args.to_string(), home);
                let args: Value = serde_json::from_str(&replaced).unwrap_or_else(|error| {
                    eprintln!(
                        "(jeeves) could not parse the tool arguments of a case ({error}); sending them to the model as written: {replaced}"
                    );
                    args.clone()
                });
                Mock::tool_use(&format!("tu_{index}"), name, args)
            }
        }
    }
}

/// The assertion set. All checks are "must contain / must not contain / sequence
/// equals" grade — every deterministic eval verdict must be reproducible, never
/// resting on another model.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Expectations {
    #[serde(default)]
    pub reply_contains: Vec<String>,
    #[serde(default)]
    pub reply_not_contains: Vec<String>,
    /// The **ordered** sequence of tool names (empty vec = unchecked; accumulated across turns).
    #[serde(default)]
    pub tools_used: Vec<String>,
    /// Fragments any tool output must contain — separates "called but failed" from "really ran through".
    #[serde(default)]
    pub tool_output_contains: Vec<String>,
    #[serde(default)]
    pub iterations: Option<i32>,
    /// Total new facts across emitted consolidation notifications.
    #[serde(default)]
    pub consolidation_new_facts: Option<i32>,
    /// How many approval questions the turn asked. `0` is a real assertion ("nothing asked"),
    /// which is what makes "the remembered rule took effect" checkable.
    #[serde(default)]
    pub approvals: Option<usize>,
    /// The event sequence this run must produce, matched as an **ordered subsequence**.
    ///
    /// Subsequence, not equality: asserting every chunk boundary would turn "one more delta" into a
    /// red light, which is noise, not a regression. Each item matches either the plain event name
    /// (`toolStarted`) or `name:detail` (`toolStarted:run_command`). Empty = unchecked, so every
    /// existing deterministic case keeps behaving exactly as before.
    #[serde(default)]
    pub events: Vec<String>,
    /// The last turn was interrupted.
    #[serde(default)]
    pub interrupted: Option<bool>,
    /// Appeared in the full text of every model request.
    #[serde(default)]
    pub prompt_contains: Vec<String>,
    /// Appeared in none of the model requests' full text.
    #[serde(default)]
    pub prompt_not_contains: Vec<String>,
    /// Appeared in the **last** model request (asks "what did the newest turn see").
    #[serde(default)]
    pub last_prompt_contains: Vec<String>,
    /// Did not appear in the last model request.
    #[serde(default)]
    pub last_prompt_not_contains: Vec<String>,
    /// The task's final status: `done` / `needs_attention` / `failed` / `suspended` …
    #[serde(default)]
    pub task_status: Option<String>,
    /// Every acceptance condition passed (`true`) or at least one failed (`false`).
    #[serde(default)]
    pub task_checks_passed: Option<bool>,
    /// How many deliverables the task recorded.
    #[serde(default)]
    pub task_artifacts: Option<usize>,
}

/// A judge case: a real model answers one turn, the referee scores by criteria. No
/// scripted replies — the real Jeeves is under test.
#[derive(Debug, Clone, Deserialize)]
pub struct JudgeScenario {
    pub id: String,
    #[serde(default)]
    pub description: String,
    pub message: String,
    pub judge: JudgeSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JudgeSpec {
    /// What this case cares about most (a custom section of the referee's rubric).
    pub criteria: String,
}

/// Reads judge cases (same JSONL shape as deterministic cases; directories and files both accepted).
pub fn load_judge(paths: &[PathBuf]) -> anyhow::Result<Vec<JudgeScenario>> {
    crate::jsonl::read_jsonl(paths, "judge case", |_, _| Ok(()))
}

/// Reads all deterministic cases: each path is a JSONL file or directory (a directory consumes its *.jsonl).
pub fn load_all(paths: &[PathBuf]) -> anyhow::Result<Vec<Scenario>> {
    crate::jsonl::read_jsonl(paths, "case", |_, _| Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_in_tool_args_stays_valid_json_for_windows_paths() {
        let home = std::path::Path::new(r"C:\Users\jeeves\AppData\Local\Temp\.tmp42");
        let scripted = ScriptedResponse::ToolUse {
            name: "read_file".to_string(),
            args: serde_json::json!({"path": "{{home}}/notes/article.md"}),
        };

        let response = scripted.to_create_response(0, home);
        let uses = response.tool_uses();
        let (_, name, input) = uses[0];
        let path = input["path"].as_str().unwrap_or_default();

        assert_eq!(name, "read_file");
        assert!(
            !path.contains("{{home}}"),
            "the placeholder was not expanded — most likely the expansion broke JSON parsing and the
             original text was kept: {path}"
        );
        assert!(
            !path.contains('\\'),
            "a fixture path must not contain backslashes: in a JSON string that is an invalid escape: {path}"
        );
        assert!(
            path.starts_with("C:/Users/jeeves/AppData/Local/Temp/.tmp42/"),
            "the expansion should be a file under the same home: {path}"
        );
        assert!(path.ends_with("/notes/article.md"), "{path}");
    }
}
