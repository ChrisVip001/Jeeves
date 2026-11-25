use super::*;
use crate::{Server, rpc_error::invalid};
use jeeves_protocol::*;
fn origin(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.username().is_empty()
            && url.password().is_none(),
        "Only HTTP(S) without embedded credentials is supported"
    );
    Ok(url.origin().ascii_serialization())
}
fn scope_origin(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value)?;
    ensure!(
        url.path() == "/" && url.query().is_none() && url.fragment().is_none(),
        "Website scope entries must be origins without paths"
    );
    origin(value)
}
impl BrowserHost {
    pub(crate) async fn list(
        &self,
        server: &Server,
        owner: &str,
    ) -> Result<BrowserListResponse, ErrorObject> {
        if server.chat.session(owner).await.map_err(invalid)?.is_none() {
            return Err(invalid("Conversation missing"));
        }
        Ok(BrowserListResponse {
            available: self.available(),
            sessions: store::list(&self.pool, owner)
                .await
                .map_err(invalid)?
                .into_iter()
                .map(Row::view)
                .collect::<Result<Vec<_>>>()
                .map_err(invalid)?,
        })
    }
    pub(crate) async fn start(
        self: &Arc<Self>,
        server: &Server,
        params: BrowserStartParams,
    ) -> Result<BrowserSessionView, ErrorObject> {
        self.start_inner(server, params).await.map_err(invalid)
    }
    async fn start_inner(
        self: &Arc<Self>,
        server: &Server,
        params: BrowserStartParams,
    ) -> Result<BrowserSessionView> {
        let _gate = self.start_gate.lock().await;
        let admission = self.executor_gate.lock().await;
        let generation = self.admitted_generation(*admission).await?;
        ensure!(
            self.available(),
            "Browser components are not installed; install the optional browser-enabled Jeeves package"
        );
        let task = server
            .tasks
            .get(&params.task_id)
            .await?
            .ok_or_else(|| anyhow!("Task no longer exists"))?;
        ensure!(
            task.status != jeeves_state::tasks::TaskStatus::Cancelled,
            "A cancelled task cannot open a browser"
        );
        let owner = task
            .session_id
            .as_deref()
            .ok_or_else(|| anyhow!("Browser requires an owning conversation"))?;
        ensure!(
            params.url.len() <= 2048 && params.origins.len() <= 11,
            "Browser scope is too large"
        );
        let mut origins = vec![origin(&params.url)?];
        for entry in &params.origins {
            let entry = scope_origin(entry)?;
            if !origins.contains(&entry) {
                origins.push(entry);
            }
        }
        {
            let mut sessions = self
                .sessions
                .lock()
                .map_err(|_| anyhow!("Browser sessions unavailable"))?;
            sessions.retain(|_, session| !session.worker.is_closed());
            ensure!(
                !sessions.contains_key(&params.task_id) && sessions.len() < 2,
                "At most two task browsers may be open; close an existing browser first"
            );
        }
        let mut row = store::create(&self.pool, &task.id, owner, &origins, &params.url).await?;
        drop(admission);
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow!("Browser not configured"))?;
        let worker = match Worker::start(config).await {
            Ok(worker) => worker,
            Err(error) => {
                row.status = "needsAttention".into();
                row.error =
                    Some("Browser worker could not start; inspect the installed components".into());
                store::update(&self.pool, &row, row.generation).await?;
                return Err(error);
            }
        };
        let session = Arc::new(Session {
            id: row.id.clone(),
            task: task.id.clone(),
            worker,
            control: tokio::sync::Mutex::new(()),
        });
        if let Err(error) = self.register(session.clone(), generation).await {
            session.worker.close();
            return Err(error);
        }
        self.heartbeat(&session);
        let reply = self
            .exchange(
                &session,
                BrowserWorkerCommand::Launch {
                    url: params.url,
                    origins,
                    allow_localhost: params.allow_localhost,
                    headless: config.headless,
                    generation: row.generation,
                },
            )
            .await?;
        if let Some(error) = reply.error {
            self.failed(&session, &error).await;
            return Err(anyhow!(error));
        }
        if store::hold_get(&self.pool, &task.id).await?.is_some() {
            let seen = self
                .exchange(&session, BrowserWorkerCommand::Observe)
                .await?;
            if let Some(error) = seen.error {
                self.failed(&session, &error).await;
                return Err(anyhow!(error));
            }
            ensure!(
                seen.state
                    .as_ref()
                    .and_then(|s| s.observation.as_ref())
                    .is_some(),
                "A fresh observation is required to resume"
            );
            store::hold_clear(&self.pool, &task.id, &session.id, row.generation).await?;
            eprintln!(
                "(jeeves) task {}: browser hold cleared after a fresh observation",
                task.id
            );
        }
        store::get(&self.pool, &task.id)
            .await?
            .ok_or_else(|| anyhow!("Browser state missing"))?
            .view()
    }
    pub(crate) async fn control(
        &self,
        params: BrowserControlParams,
    ) -> Result<BrowserSessionView, ErrorObject> {
        self.control_inner(params).await.map_err(invalid)
    }
    async fn control_inner(&self, params: BrowserControlParams) -> Result<BrowserSessionView> {
        let session = self.session(&params.task_id)?;
        let _control = session.control.lock().await;
        let row = store::get(&self.pool, &params.task_id)
            .await?
            .ok_or_else(|| anyhow!("Browser state missing"))?;
        ensure!(
            row.id == session.id && row.generation == params.generation,
            "Browser control changed; refresh before acting"
        );
        ensure!(
            params.action != BrowserControlAction::Takeover
                || session.worker.remote_supported()
                || !self.config.as_ref().is_some_and(|config| config.headless),
            "A headless test browser has no human window"
        );
        let new_origin = if params.action == BrowserControlAction::AllowOrigin {
            ensure!(
                row.status == "human",
                "Take control before changing the allowed websites"
            );
            Some(scope_origin(
                params
                    .origin
                    .as_deref()
                    .ok_or_else(|| anyhow!("Website origin required"))?,
            )?)
        } else {
            ensure!(
                params.origin.is_none(),
                "Only AllowOrigin accepts a website origin"
            );
            None
        };
        let generation = row
            .generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("Browser generation exhausted"))?;
        let changed=sqlx::query("UPDATE browser_sessions SET generation=?,status='pausing' WHERE task_id=? AND id=? AND generation=?").bind(generation).bind(&row.task_id).bind(&row.id).bind(row.generation).execute(&self.pool).await?.rows_affected();
        ensure!(
            changed == 1,
            "Browser control changed; refresh before acting"
        );
        let reply = self
            .exchange(
                &session,
                BrowserWorkerCommand::Control {
                    generation,
                    action: params.action,
                    origin: new_origin.clone(),
                },
            )
            .await?;
        if let Some(error) = reply.error {
            self.failed(&session, &error).await;
            return Err(anyhow!(error));
        }
        if let Some(new_origin) = new_origin {
            let mut origins: Vec<String> = serde_json::from_str(&row.origins_json)?;
            if !origins.contains(&new_origin) {
                origins.push(new_origin);
            }
            let changed = sqlx::query("UPDATE browser_sessions SET origins_json=? WHERE id=? AND generation=? AND status='human'")
                .bind(serde_json::to_string(&origins)?)
                .bind(&row.id)
                .bind(generation)
                .execute(&self.pool)
                .await?.rows_affected();
            ensure!(
                changed == 1,
                "Browser control changed before saving the origin"
            );
        }
        if params.action == BrowserControlAction::Close {
            self.sessions
                .lock()
                .map_err(|_| anyhow!("Browser sessions unavailable"))?
                .retain(|_, current| current.id != session.id);
        }
        store::get(&self.pool, &row.task_id)
            .await?
            .ok_or_else(|| anyhow!("Browser state missing"))?
            .view()
    }
    pub(crate) async fn frame(&self, task: &str) -> Result<BrowserFrameResponse, ErrorObject> {
        let session = self.session(task).map_err(invalid)?;
        let reply = self
            .exchange(&session, BrowserWorkerCommand::Frame)
            .await
            .map_err(invalid)?;
        Self::preview(reply).map_err(invalid)
    }
    pub(super) fn preview(reply: BrowserWorkerResponse) -> Result<BrowserFrameResponse> {
        if let Some(error) = reply.error {
            return Err(anyhow!(error));
        }
        let frame = reply
            .frame
            .ok_or_else(|| anyhow!("Browser has no frame yet"))?;
        if frame.data_url.len() > 1_100_000
            || !frame.data_url.starts_with("data:image/jpeg;base64,")
        {
            return Err(anyhow!("Invalid browser frame"));
        }
        Ok(frame)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_rejects_credentials_protocols_and_paths() {
        assert_eq!(
            origin("https://example.com/page").unwrap(),
            "https://example.com"
        );
        for value in [
            "file:///tmp/a",
            "https://secret@example.com",
            "https://example.com/path",
            "https://example.com/?q=x",
        ] {
            assert!(scope_origin(value).is_err(), "{value}");
        }
    }
}
