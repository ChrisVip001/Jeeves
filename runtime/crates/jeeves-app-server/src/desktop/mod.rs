//! A human-selected native window belongs to one task and one live helper.
mod screen;
mod worker;
use crate::Server;
use anyhow::{Result, anyhow, bail, ensure};
use jeeves_protocol::*;
use std::sync::{Arc, Mutex};
use worker::Worker;
type Cmd = DesktopWorkerCommand;
type Reply = DesktopWorkerReply;

#[derive(Default)]
pub(crate) struct DesktopHost(Mutex<Option<Arc<Window>>>);
pub(crate) struct Window {
    task: String,
    owner: Option<String>,
    worker: Worker,
    selected: std::sync::OnceLock<()>,
    state: tokio::sync::Mutex<WindowState>,
}
#[derive(Default)]
struct WindowState {
    observation: Option<DesktopObservation>,
    frame: Option<screen::Preview>,
}
impl DesktopHost {
    fn current(&self) -> Option<Arc<Window>> {
        self.0.lock().expect("window lock").clone()
    }
    fn window(&self, task: &str) -> Result<Arc<Window>> {
        self.current()
            .filter(|w| w.task == task && w.selected.get().is_some() && !w.worker.is_closing())
            .ok_or_else(|| anyhow!("Select a window for this task first"))
    }
    fn check(&self, w: &Arc<Window>) -> Result<()> {
        ensure!(
            !w.worker.is_closing() && self.current().is_some_and(|live| Arc::ptr_eq(&live, w)),
            "Window selection changed"
        );
        Ok(())
    }
    pub(crate) fn stop_matching(
        &self,
        predicate: impl FnOnce(&Window) -> bool,
    ) -> Option<Arc<Window>> {
        let live = self.0.lock().expect("window lock");
        let window = live.as_ref().filter(|w| predicate(w))?;
        window.worker.close();
        Some(window.clone())
    }
    pub(crate) async fn finish_stop(&self, window: Option<Arc<Window>>) -> Result<()> {
        if let Some(window) = window {
            window.worker.wait().await?;
            let mut live = self.0.lock().expect("window lock");
            if live.as_ref().is_some_and(|w| Arc::ptr_eq(w, &window)) {
                live.take();
            }
        }
        Ok(())
    }
    pub(crate) async fn revoke(&self, task: Option<&str>, owner: Option<&str>) -> Result<()> {
        self.finish_stop(self.stop_matching(|w| {
            task.is_none_or(|t| t == w.task) && owner.is_none_or(|o| Some(o) == w.owner.as_deref())
        }))
        .await
    }
    async fn retire(&self, w: &Arc<Window>) -> Result<()> {
        self.finish_stop(self.stop_matching(|live| std::ptr::eq(live, w.as_ref())))
            .await
    }
    async fn exchange(&self, w: &Arc<Window>, cmd: Cmd) -> Result<Reply> {
        self.check(w)?;
        let result = w.worker.request(cmd).await;
        self.check(w)?;
        match result {
            Ok(Reply::Reselect { message }) => {
                self.retire(w).await?;
                Ok(Reply::Denied { message })
            }
            Ok(Reply::Unknown { message }) => {
                self.retire(w).await?;
                bail!("{message}")
            }
            Err(err) => {
                self.retire(w).await?;
                Err(err)
            }
            other => other,
        }
    }
}
fn observed(reply: Reply, state: &mut WindowState) -> Result<DesktopObservation> {
    state.frame = None;
    match reply {
        Reply::Observed { observation } => {
            ensure!(
                !observation.observation_id.is_empty()
                    && observation.observation_id.len() <= 128
                    && observation.elements.len() <= 128,
                "Invalid desktop observation"
            );
            state.observation = Some(observation.clone());
            Ok(observation)
        }
        Reply::Denied { message } => bail!("{message}"),
        _ => bail!("Desktop helper omitted its observation"),
    }
}
impl Server {
    async fn desktop_task(&self, task: &str) -> Result<jeeves_state::TaskRow> {
        self.tasks
            .get(task)
            .await?
            .ok_or_else(|| anyhow!("Unknown task"))
    }
    async fn active_desktop_task(&self, task: &str) -> Result<jeeves_state::TaskRow> {
        let row = self.desktop_task(task).await?;
        ensure!(
            !row.status.is_terminal() && row.status != jeeves_state::TaskStatus::Suspended,
            "Task is paused or ended"
        );
        Ok(row)
    }
    pub(crate) async fn desktop_status(&self, p: TaskGetParams) -> Result<DesktopStatus> {
        self.desktop_task(&p.task_id).await?;
        let rows:Vec<(String,String,String)> = sqlx::query_as(
            "SELECT id,status,action FROM desktop_receipts WHERE task_id=? ORDER BY rowid DESC LIMIT 20"
        ).bind(&p.task_id).fetch_all(&self.pool).await?;
        let receipts = rows
            .into_iter()
            .map(|(id, status, a)| {
                Ok(DesktopReceipt {
                    id,
                    status,
                    action: serde_json::from_str(&a)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(DesktopStatus {
            available: self
                .settings()
                .desktop_helper
                .is_some_and(|p| p.is_absolute() && p.is_file()),
            selected: self.browser.desktop.window(&p.task_id).is_ok(),
            screen_profile: self.screen_profile().await?,
            receipts,
        })
    }
    pub(crate) async fn desktop_select(&self, p: TaskGetParams) -> Result<DesktopObservation> {
        let window = {
            let control = self.session_write_lock(&format!("task-control:{}", p.task_id));
            let _control = control.lock().await;
            let pending = self.browser.executor_gate.lock().await;
            self.browser.admitted_generation(*pending).await?;
            let task = self.active_desktop_task(&p.task_id).await?;
            let owner = task
                .session_id
                .as_deref()
                .map(|s| self.permissions.owner(s))
                .transpose()
                .map_err(anyhow::Error::msg)?;
            let helper = self
                .settings()
                .desktop_helper
                .ok_or_else(|| anyhow!("Desktop helper is not installed"))?;
            let mut live = self.browser.desktop.0.lock().expect("window lock");
            ensure!(
                live.is_none(),
                "Take over the selected window before selecting another"
            );
            let window = Arc::new(Window {
                task: p.task_id,
                owner,
                worker: Worker::start(&helper)?,
                selected: Default::default(),
                state: Default::default(),
            });
            *live = Some(window.clone());
            window
        };
        let mut state = window.state.lock().await;
        let result = self
            .browser
            .desktop
            .exchange(&window, Cmd::Select)
            .await
            .and_then(|r| observed(r, &mut state));
        if result.is_ok() {
            window.selected.get_or_init(|| ());
        } else {
            self.browser.desktop.retire(&window).await?;
        }
        result
    }
    pub(crate) async fn desktop_observe(&self, p: TaskGetParams) -> Result<DesktopObservation> {
        self.active_desktop_task(&p.task_id).await?;
        let w = self.browser.desktop.window(&p.task_id)?;
        let mut state = w.state.lock().await;
        *state = WindowState::default();
        observed(
            self.browser.desktop.exchange(&w, Cmd::Observe).await?,
            &mut state,
        )
    }
    pub(crate) async fn desktop_act(&self, p: DesktopActParams) -> Result<DesktopObservation> {
        let w = self.browser.desktop.window(&p.task_id)?;
        let mut state = w.state.lock().await;
        self.active_desktop_task(&p.task_id).await?;
        let observation = state
            .observation
            .as_ref()
            .filter(|o| o.observation_id == p.observation_id)
            .ok_or_else(|| anyhow!("Observe the window again"))?;
        ensure!(
            observation
                .elements
                .iter()
                .any(|e| e.id == p.element_id && e.actions.contains(&p.action)),
            "Action is outside the observed controls"
        );
        ensure!(
            match (p.action, &p.text) {
                (DesktopAction::SetText, Some(s)) =>
                    s.len() <= 2048 && !s.chars().any(char::is_control),
                (DesktopAction::SetText, None) => false,
                (_, s) => s.is_none(),
            },
            "Expected plain text only for a text field"
        );
        *state = WindowState::default();
        sqlx::query(
            "INSERT INTO desktop_receipts(id,task_id,status,action) VALUES(?,?,'unknown',?)",
        )
        .bind(&p.observation_id)
        .bind(&p.task_id)
        .bind(serde_json::to_string(&p.action)?)
        .execute(&self.pool)
        .await?;
        let reply = self
            .browser
            .desktop
            .exchange(
                &w,
                Cmd::Act {
                    observation_id: p.observation_id.clone(),
                    element_id: p.element_id,
                    action: p.action,
                    text: p.text,
                },
            )
            .await?;
        let rejected = matches!(reply, Reply::Denied { .. });
        let result = observed(reply, &mut state);
        if result.is_err() && !rejected {
            return result;
        }
        let status = if rejected { "rejected" } else { "confirmed" };
        let _gate = self.browser.executor_gate.lock().await;
        if !rejected {
            self.browser.desktop.check(&w)?;
        }
        sqlx::query("UPDATE desktop_receipts SET status=? WHERE id=?")
            .bind(status)
            .bind(p.observation_id)
            .execute(&self.pool)
            .await?;
        result
    }
    pub(crate) async fn desktop_takeover(&self, p: TaskGetParams) -> Result<DesktopStatus> {
        let _gate = self.browser.executor_gate.lock().await;
        self.browser.desktop.revoke(Some(&p.task_id), None).await?;
        self.desktop_status(p).await
    }
}
