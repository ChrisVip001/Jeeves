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
fn cdp_endpoint(value: Option<String>) -> Result<Option<String>> {
    let Some(endpoint) = value else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(&endpoint)?;
    ensure!(
        matches!(url.scheme(), "http" | "ws"),
        "CDP endpoint must use http or ws on a loopback address"
    );
    ensure!(
        matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]")),
        "Only a literal loopback CDP endpoint is allowed"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "CDP endpoint must not carry credentials"
    );
    Ok(Some(endpoint))
}
fn profile_id(value: Option<String>) -> Result<Option<String>> {
    let Some(id) = value else { return Ok(None) };
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "A browser profile id must be 1-64 letters, digits, '-' or '_'"
    );
    Ok(Some(id))
}
fn profile_account(owner: &str, id: &str) -> String {
    format!("browser-profile:{owner}:{id}")
}
fn profile_get(host: &std::path::Path, owner: &str, id: &str) -> Option<String> {
    jeeves_config::models::SecretStore::auto().get(host, &profile_account(owner, id))
}
fn profile_put(host: &std::path::Path, owner: &str, id: &str, state: &str) -> Result<()> {
    jeeves_config::models::SecretStore::auto()
        .put(host, &profile_account(owner, id), state)
        .map_err(|error| anyhow!("Could not save browser profile: {error}"))?;
    Ok(())
}
fn profile_forget(host: &std::path::Path, owner: &str, id: &str) -> Result<()> {
    jeeves_config::models::SecretStore::auto()
        .remove(host, &profile_account(owner, id))
        .map_err(|error| anyhow!("Could not forget browser profile: {error}"))
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
        let (kind, cdp_endpoint) = match params.provider.clone() {
            Some(provider) => (provider.kind, cdp_endpoint(provider.endpoint)?),
            None => (BrowserProviderKind::Local, None),
        };
        if kind == BrowserProviderKind::CdpAttach {
            ensure!(
                self.config.as_ref().is_some_and(|c| c.cdp_attach),
                "Loopback CDP attach is disabled; set JEEVES_BROWSER_CDP_ATTACH=1"
            );
            ensure!(cdp_endpoint.is_some(), "CDP attach requires an endpoint");
            ensure!(
                params.profile_id.is_none(),
                "An attached browser owns its login state; do not pass a profile id"
            );
        } else {
            ensure!(
                cdp_endpoint.is_none(),
                "Only a CDP attach provider accepts an endpoint"
            );
        }
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
        let profile_id = profile_id(params.profile_id.clone())?;
        let storage_state = profile_id
            .as_deref()
            .and_then(|id| profile_get(&self.home, owner, id));
        let mut row = store::create(
            &self.pool,
            &task.id,
            owner,
            &origins,
            &params.url,
            profile_id.as_deref(),
        )
        .await?;
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
                    storage_state,
                    cdp_endpoint,
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
        if matches!(
            params.action,
            BrowserControlAction::SaveProfile | BrowserControlAction::ForgetProfile
        ) {
            ensure!(
                row.status == "human",
                "Take control before changing the login profile"
            );
            ensure!(
                row.profile_id.is_some(),
                "Open a browser with a profile id before saving or forgetting it"
            );
        }
        if params.action == BrowserControlAction::ForgetProfile {
            let owner: String =
                sqlx::query_scalar("SELECT owner_session_id FROM browser_sessions WHERE id=?")
                    .bind(&row.id)
                    .fetch_one(&self.pool)
                    .await?;
            let id = row
                .profile_id
                .as_deref()
                .ok_or_else(|| anyhow!("This browser has no profile to forget"))?;
            profile_forget(&self.home, &owner, id)?;
            return row.view();
        }
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
        if params.action == BrowserControlAction::SaveProfile {
            let state = reply
                .profile
                .ok_or_else(|| anyhow!("The browser did not return a login profile"))?;
            let owner: String =
                sqlx::query_scalar("SELECT owner_session_id FROM browser_sessions WHERE id=?")
                    .bind(&row.id)
                    .fetch_one(&self.pool)
                    .await?;
            let id = row
                .profile_id
                .as_deref()
                .ok_or_else(|| anyhow!("This browser has no profile id"))?;
            profile_put(&self.home, &owner, id, &state)?;
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
    fn cdp_endpoint_accepts_only_literal_loopback() {
        for value in [
            "http://127.0.0.1:9222",
            "ws://127.0.0.1:9222/devtools/browser/x",
            "http://[::1]:9222",
            "ws://[::1]:9222",
        ] {
            assert!(
                cdp_endpoint(Some(value.to_string())).is_ok(),
                "expected loopback endpoint: {value}"
            );
        }
        for value in [
            "http://localhost:9222",
            "https://127.0.0.1:9222",
            "http://user@127.0.0.1:9222",
            "http://192.168.1.2:9222",
        ] {
            assert!(
                cdp_endpoint(Some(value.to_string())).is_err(),
                "unexpected endpoint accepted: {value}"
            );
        }
    }
    #[test]
    fn profile_account_is_owner_scoped() {
        assert_ne!(
            profile_account("owner-a", "work"),
            profile_account("owner-b", "work")
        );
        assert!(profile_account("owner-a", "work").contains("owner-a"));
    }
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
