use crate::{approval, ActiveTurn, EventSink, FollowUp, Server};
use anyhow::Result;
use jeeves_loop::{Interrupt, Steering};
use jeeves_protocol::{RememberScope, SettingsPatch};
use jeeves_tools::approval::RememberTarget;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;

/// The actor recorded when the Runtime's own human answers (no channel identity given).
pub(crate) const OWNER: &str = "owner";

impl Server {
    /// The session's write-section lock — created on first use, held forever.
    /// Two hash lookups are nothing next to what the lock actually protects.
    pub(crate) fn session_write_lock(&self, session_id: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self
            .session_locks
            .lock()
            .expect("the session_locks should not be poisoned");
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// Dashboard read surface: the session's todo sheet. `None` = never written.
    pub fn todo_view(&self, session_id: &str) -> Option<jeeves_protocol::TodoListView> {
        self.todo
            .read(session_id)
            .map(|list| jeeves_protocol::TodoListView {
                revision: list.revision as i64,
                items: list
                    .items
                    .into_iter()
                    .map(|item| jeeves_protocol::TodoItemView {
                        content: item.content,
                        status: item.status.as_str().to_string(),
                    })
                    .collect(),
            })
    }

    /// This turn's approval channel. `never` mode (the default) returns `None` — nobody asks, so nobody
    /// answers, and actions requiring approval are rejected outright.
    pub(crate) fn approval_bridge(
        &self,
        turn_id: &str,
        sink: &EventSink,
    ) -> Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>> {
        let settings = self.settings();
        if settings.approval != "on-request" {
            return None;
        }
        self.approval_bridge_forced(turn_id, sink)
    }

    pub(crate) fn approval_bridge_forced(
        &self,
        turn_id: &str,
        sink: &EventSink,
    ) -> Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>> {
        let settings = self.settings();
        Some(Arc::new(approval::Bridge {
            pending: self.approvals.clone(),
            push: settings.web_push_contact.as_ref().map(|_| self.clone()),
            sink: sink.clone(),
            turn_id: turn_id.to_string(),
            timeout_secs: settings.approval_timeout_secs,
            notify: self.notifier.clone(),
            logbook: self.approval_log.clone(),
            grants: self.grants.clone(),
            runs: self.runs.clone(),
            events: self.events.clone(),
            session_id: self
                .turns
                .lock()
                .ok()
                .and_then(|turns| turns.get(turn_id).map(|t| t.session_id.clone()))
                .unwrap_or_else(|| turn_id.to_string()),
            restored: self.restored_approvals.clone(),
            resume_yes: self.resume_yes.clone(),
        }))
    }

    /// Answer one approval request. Returns `false` = too late (timed out, or that turn already ended) —
    /// tell the client honestly, so it does not believe the approval took effect.
    ///
    /// `remember` says how long the yes lasts. It is applied **before** the answer is delivered, so the
    /// tool that asked (and the next call after it) already sees the new rule — an approval that only
    /// takes effect after a restart is not what the click promised.
    pub fn answer_approval(
        &self,
        turn_id: &str,
        request_id: &str,
        approved: bool,
        remember: RememberScope,
    ) -> bool {
        self.answer_approval_as(turn_id, request_id, approved, remember, OWNER)
    }

    /// `approval/respond`: the same answer, but refused with a text error when the client answers
    /// a version of the question that is no longer current (a restart re-issued it).
    pub fn answer_approval_checked(
        &self,
        turn_id: &str,
        request_id: &str,
        approved: bool,
        remember: RememberScope,
        version: Option<i32>,
        actor: &str,
    ) -> Result<bool, String> {
        if let Some(sent) = version {
            let current = {
                let table = self
                    .approvals
                    .lock()
                    .expect("the approvals lock should not be poisoned");
                table
                    .get(turn_id)
                    .and_then(|per_turn| per_turn.get(request_id))
                    .map(|w| w.request.version)
            };
            let current = current.or_else(|| {
                self.restored_approvals
                    .lock()
                    .ok()?
                    .iter()
                    .find(|r| r.turn_id == turn_id && r.request_id == request_id)
                    .map(|r| r.version as i32)
            });
            if let Some(current) = current.filter(|current| *current != sent) {
                return Err(format!(
                    "Error: this approval is now at version {current}, not {sent}; reload the pending list and answer the current question"
                ));
            }
        }
        let leftover = {
            let mut rows = self
                .restored_approvals
                .lock()
                .expect("the restored approvals lock should not be poisoned");
            rows.iter()
                .position(|r| r.turn_id == turn_id && r.request_id == request_id)
                .map(|at| rows.remove(at))
        };
        if let Some(row) = leftover {
            if approved {
                self.resume_yes
                    .lock()
                    .expect("the resume yes lock should not be poisoned")
                    .insert(crate::approval::yes_key(
                        &row.turn_id,
                        &row.tool,
                        &row.args_preview,
                    ));
            }
            let grants = self.grants.clone();
            tokio::spawn(async move {
                let _ = grants.pending_remove(&row.turn_id, &row.request_id).await;
            });
            return Ok(true);
        }
        Ok(self.answer_approval_as(turn_id, request_id, approved, remember, actor))
    }

    /// The same answer, recording who gave it (`owner`, or a channel identity like `lark:ou_…`).
    pub fn answer_approval_as(
        &self,
        turn_id: &str,
        request_id: &str,
        approved: bool,
        remember: RememberScope,
        actor: &str,
    ) -> bool {
        let waiting = self
            .approvals
            .lock()
            .expect("the approvals lock should not be poisoned")
            .get_mut(turn_id)
            .and_then(|per_turn| per_turn.remove(request_id));
        let Some(waiting) = waiting else {
            return false;
        };
        if waiting.remaining().is_zero() {
            return false;
        }
        if approved {
            self.remember(waiting.remember.as_ref(), remember);
        }
        waiting.tx.send((approved, actor.to_string())).is_ok()
    }

    /// Write the yes where it belongs, according to its scope.
    ///
    /// * `once` — nowhere: the next identical action asks again.
    /// * `session` — the live in-process cell only (never the disk).
    /// * `always` — the live cell **and** `settings.json`, so it survives a restart.
    ///
    /// A remembered rule is not a fence with a hole in it: `run_command` still meets hard-deny and the
    /// sandbox, and a write still has to be inside the writable boundary.
    fn remember(&self, target: Option<&RememberTarget>, scope: RememberScope) {
        let Some(target) = target else {
            return;
        };
        if scope == RememberScope::Once {
            return;
        }
        match target {
            RememberTarget::Command(command) => {
                push_unique(&self.exec_session_allow, command.trim().to_string());
                if scope == RememberScope::Always {
                    self.persist_exec_allow(command.trim());
                }
            }
            RememberTarget::Directory(dir) => {
                push_unique(&self.write_approved_dirs, dir.clone());
                if scope == RememberScope::Always {
                    self.persist_write_dir(dir);
                }
            }
        }
    }

    /// "remember this one" for a command: replace the allow table wholesale and persist into `settings.json`.
    ///
    /// What is remembered is **this command itself**, no wildcards — what the user saw and approved was
    /// it, not the whole class. The live table was already updated by `remember`, so this only has to
    /// make it survive a restart.
    fn persist_exec_allow(&self, command: &str) {
        let mut settings = self.settings();
        if settings.exec_allow.iter().any(|rule| rule == command) {
            return;
        }
        settings.exec_allow.push(command.to_string());
        let patch = SettingsPatch {
            exec_allow: Some(settings.exec_allow.clone()),
            ..Default::default()
        };
        match self.apply_config_patch(&patch) {
            Ok(_) => eprintln!("(jeeves) remembered this command: {command} (written to settings.json; effective now)"),
            Err(why) => eprintln!("(jeeves) could not remember this command: {why}"),
        }
    }

    /// "remember this one" for a write: remember the **directory**, never the file name.
    ///
    /// The directory is a "don't ask again" note, not a writable root — containment still decides
    /// whether a write may land there at all.
    fn persist_write_dir(&self, dir: &std::path::Path) {
        let mut settings = self.settings();
        let entry = dir.display().to_string();
        if settings
            .write_allow_dirs
            .iter()
            .any(|known| known.as_path() == dir)
        {
            return;
        }
        settings.write_allow_dirs.push(dir.to_path_buf());
        let patch = SettingsPatch {
            write_allow_dirs: Some(
                settings
                    .write_allow_dirs
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect(),
            ),
            ..Default::default()
        };
        match self.apply_config_patch(&patch) {
            Ok(_) => {
                eprintln!("(jeeves) remembered this directory: {entry} (writing here will not ask each time)")
            }
            Err(why) => eprintln!("(jeeves) could not remember this directory: {why}"),
        }
    }

    /// Claim a session for one turn, handing out its cancellation token and steering channel.
    ///
    /// **One driver per session.** Two turns on one session would interleave their write
    /// sections and each would see half the other's history — before 0.16 nothing stopped it.
    /// `Err` carries the turn that holds the session: the only part of the answer the caller
    /// can act on ("wait for it, or steer it").
    pub(crate) fn begin_turn(
        &self,
        turn_id: &str,
        session_id: &str,
    ) -> Result<(Arc<Interrupt>, Arc<Mutex<Steering>>), String> {
        let mut turns = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned");
        if let Some((holder, _)) = turns.iter().find(|(_, turn)| turn.session_id == session_id) {
            return Err(holder.clone());
        }
        let interrupt = Interrupt::new();
        let steering = Steering::new();
        turns.insert(
            turn_id.to_string(),
            ActiveTurn {
                interrupt: interrupt.clone(),
                session_id: session_id.to_string(),
                started_at: chrono::Local::now().to_rfc3339(),
                steering: steering.clone(),
            },
        );
        Ok((interrupt, steering))
    }

    /// The per-tenant cap on simultaneous turns (`JEEVES_MAX_TURNS`, 0 = none). `Err` carries the
    /// reason the caller shows: a turn over the cap is refused, never queued behind the user's back.
    pub(crate) fn turn_capacity(&self) -> Result<(), String> {
        let cap = self.settings().max_concurrent_turns;
        if cap <= 0 {
            return Ok(());
        }
        let running = self.active_turns(None).len();
        if running >= cap as usize {
            return Err(format!(
                "{running} turns are already running (limit {cap}, JEEVES_MAX_TURNS); try again when one finishes"
            ));
        }
        Ok(())
    }

    /// The per-tenant cap on the home directory (`JEEVES_HOME_MAX_MB`, 0 = none). Over the cap a new turn is
    /// refused; reads, exports and cleanup keep working, so the owner can always make room.
    pub(crate) async fn home_capacity(&self) -> Result<(), String> {
        let settings = self.settings();
        if settings.home_max_mb <= 0 {
            return Ok(());
        }
        let cap = settings.home_max_mb as u64;
        let home = settings.home.clone();
        let used = tokio::task::spawn_blocking(move || Self::dir_bytes(&home))
            .await
            .unwrap_or(0);
        if used >= cap * 1024 * 1024 {
            return Err(format!(
                "the home directory uses {} MB (limit {cap} MB, JEEVES_HOME_MAX_MB); free some space before starting a new turn",
                used / (1024 * 1024)
            ));
        }
        Ok(())
    }

    /// Total bytes of the regular files under `path`; symlinks are not followed, unreadable entries count as 0.
    fn dir_bytes(path: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        let mut total = 0;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            total += if meta.is_dir() {
                Self::dir_bytes(&entry.path())
            } else {
                meta.len()
            };
        }
        total
    }

    /// The steering channel of a running turn — what `turn/start` hands to the loop and what the
    /// transcript reads afterwards (a message accepted just before the end must still be logged).
    pub(crate) fn steering_of(&self, turn_id: &str) -> Option<Arc<Mutex<Steering>>> {
        self.turns
            .lock()
            .expect("the turns lock should not be poisoned")
            .get(turn_id)
            .map(|turn| turn.steering.clone())
    }

    /// Add one sentence to the turn running in this session.
    ///
    /// `Err` = nothing is running (the caller should `turn/start` instead — falling through to a
    /// new turn silently would be a lie about what happened). `Ok` carries the turn id and how
    /// many additions it has accepted, this one included.
    pub(crate) fn steer(&self, session_id: &str, message: &str) -> Result<(String, i32), ()> {
        let turns = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned");
        let Some((turn_id, turn)) = turns.iter().find(|(_, turn)| turn.session_id == session_id)
        else {
            return Err(());
        };
        let mut steering = turn
            .steering
            .lock()
            .expect("the steering lock should not be poisoned");
        let mut approvals = self.approvals.lock().expect("approvals lock");
        steering.push(message);
        if let Some(waiting) = approvals.get_mut(turn_id) {
            let sends: Vec<_> = waiting
                .iter()
                .filter(|(_, w)| matches!(w.request.tool.as_str(), "email_send" | "feishu_send"))
                .map(|(id, _)| id.clone())
                .collect();
            for id in sends {
                if let Some(request) = waiting.remove(&id) {
                    let _ = request.tx.send((false, "task revision".into()));
                }
            }
        }
        Ok((turn_id.clone(), steering.all().len() as i32))
    }

    /// Queue a message to run **after** the turn in flight (0.16 follow-up).
    ///
    /// Refused when nothing is running: "when you are done" with nothing to be done is just
    /// "now", and the caller should `turn/start` and watch its stream instead of queueing into
    /// an idle session that would never drain.
    pub(crate) async fn follow_up(&self, session_id: &str, message: &str) -> FollowUp {
        if self.active_turns(Some(session_id)).is_empty() {
            return FollowUp::Idle;
        }
        match self.queue.push(session_id, message).await {
            Ok(position) => FollowUp::Queued(position as i32),
            Err(e) => FollowUp::Failed(e.to_string()),
        }
    }

    /// The turns running right now (optionally for one session), oldest first — a list that
    /// reshuffles between two polls reads as "something changed" when nothing did.
    pub(crate) fn active_turns(&self, session: Option<&str>) -> Vec<(String, String, String)> {
        let turns = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned");
        let mut rows: Vec<(String, String, String)> = turns
            .iter()
            .filter(|(_, turn)| session.is_none_or(|id| turn.session_id == id))
            .map(|(id, turn)| (id.clone(), turn.session_id.clone(), turn.started_at.clone()))
            .collect();
        rows.sort_by(|a, b| a.2.cmp(&b.2));
        rows
    }

    /// Remove a turn. Once finished, there is nothing left to interrupt.
    pub(crate) fn finish_turn(&self, turn_id: &str) {
        let mut turns = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned");
        self.suspending
            .lock()
            .expect("the suspending lock should not be poisoned")
            .remove(turn_id);
        turns.remove(turn_id);
        drop(turns);
        self.approvals
            .lock()
            .expect("the approvals lock should not be poisoned")
            .remove(turn_id);
    }

    /// A human pressed pause on the turn a task carries: mark it **and** interrupt it, in one
    /// step. The mark only lands when a live turn exists — otherwise a stale flag would make
    /// the *next* turn (a resume!) think it was paused, and finalize would keep it suspended.
    /// Returns false = no live turn (the task was idle; nothing to interrupt).
    pub(crate) fn suspend_turn(&self, turn_id: &str) -> bool {
        let turns = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned");
        let Some(turn) = turns.get(turn_id) else {
            return false;
        };
        self.suspending
            .lock()
            .expect("the suspending lock should not be poisoned")
            .insert(turn_id.to_string());
        turn.interrupt.cancel();
        true
    }

    /// Was this turn paused rather than stopped? Consumes the flag: one verdict per turn.
    pub(crate) fn take_suspending(&self, turn_id: &str) -> bool {
        self.suspending
            .lock()
            .expect("the suspending lock should not be poisoned")
            .remove(turn_id)
    }

    /// Interrupt a running turn. Returns false = not found (already finished).
    /// Exposed for eval: the interrupt scenario needs to flip the token mid-stream.
    pub fn interrupt_turn(&self, turn_id: &str) -> bool {
        let found = self
            .turns
            .lock()
            .expect("the turns lock should not be poisoned")
            .get(turn_id)
            .map(|turn| turn.interrupt.clone());
        match found {
            Some(interrupt) => {
                interrupt.cancel();
                true
            }
            None => false,
        }
    }
}

/// Add `value` to a shared list unless it is already there (an approval answered twice in one turn
/// must not grow the table).
///
/// A poisoned lock is recovered rather than propagated: remembering is a convenience, and a thread
/// that panicked elsewhere must not be able to take the turn down here.
pub(crate) fn push_unique<T: PartialEq>(cell: &Mutex<Vec<T>>, value: T) {
    let mut list = match cell.lock() {
        Ok(list) => list,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !list.contains(&value) {
        list.push(value);
    }
}

impl Server {
    /// Assembled from state.db. A provider-resolution failure is not fatal — record the reason and report it at turn time.
    /// Set or clear the goal on this session's task. **Only a human can reach here** — the model
    /// has no task tool, and the REPL's `/goal` is the only caller.
    ///
    /// A goal is a column on the row (`agent_tasks.goal_condition`), not a record of its own, so
    /// "set a goal" is either "create that task" or "edit it". Clearing keeps the row and its
    /// title: the task survives as a single-turn one. Returns the row as it now reads, so the
    /// caller prints what the ledger says instead of what it hoped.
    pub async fn set_task_goal(
        &self,
        session_id: &str,
        condition: Option<&str>,
    ) -> Option<jeeves_state::TaskRow> {
        let trimmed = condition.map(str::trim).filter(|text| !text.is_empty());
        let active = self.tasks.active(session_id).await.ok().flatten();
        match (trimmed, active) {
            (Some(condition), None) => {
                let max_rounds = self
                    .settings
                    .read()
                    .expect("the settings lock should not be poisoned")
                    .goal_max_rounds;
                let id = format!("task{}", chrono::Local::now().timestamp_millis());
                if let Err(e) = self
                    .tasks
                    .create(jeeves_state::NewTask {
                        id: &id,
                        session_id: Some(session_id),
                        title: condition,
                        goal_condition: Some(condition),
                        acceptance_json: "[]",
                        budget_json: "{}",
                        cwd: None,
                        max_rounds,
                        tier: None,
                    })
                    .await
                {
                    eprintln!("(jeeves) the goal could not be turned into a task: {e}");
                    return None;
                }
            }
            (Some(condition), Some(task)) => {
                if let Err(e) = self.tasks.set_goal(&task.id, Some(condition)).await {
                    eprintln!("(jeeves) the goal was not written to the task ledger: {e}");
                }
            }
            (None, Some(task)) => {
                if let Err(e) = self.tasks.set_goal(&task.id, None).await {
                    eprintln!("(jeeves) clearing the goal was not written to the task ledger: {e}");
                }
            }
            (None, None) => return None,
        }
        self.tasks.active(session_id).await.ok().flatten()
    }
}
