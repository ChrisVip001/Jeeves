//! Session policy is human-controlled, durable, and checked at each tool boundary.
use crate::{EventSink, Server};
use jeeves_protocol::{
    ErrorObject, PermissionMode, SessionPermissionResponse, SessionPermissionSetParams, codes,
};
use jeeves_tools::{
    ToolCtx,
    approval::{ApprovalOutcome, ApprovalRequest},
    permissions::{Authorization, AuthorizationFuture, ExecutionPolicy, proven_read_call, redact},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

pub(crate) struct Policies {
    rows: Mutex<Option<HashMap<String, (PermissionMode, i32)>>>,
    updates: tokio::sync::Mutex<()>,
    owners: Mutex<HashMap<String, String>>,
    pool: sqlx::SqlitePool,
}
impl Policies {
    pub(crate) async fn load(pool: sqlx::SqlitePool) -> Arc<Self> {
        let rows = match jeeves_state::session_permissions::load(&pool).await {
            Ok(rows) => rows
                .into_iter()
                .map(|row| {
                    let mode = serde_json::from_value(serde_json::Value::String(row.mode))
                        .map_err(|e| e.to_string())?;
                    Ok((row.session_id, (mode, row.revision)))
                })
                .collect::<Result<HashMap<_, _>, String>>()
                .map_err(|error| {
                    eprintln!("(jeeves) invalid stored session policy; tools blocked: {error}")
                })
                .ok(),
            Err(error) => {
                eprintln!(
                    "(jeeves) cannot load session permissions; tool execution blocked: {error}"
                );
                None
            }
        };
        let owners = sqlx::query_as::<_, (String, String)>("SELECT d.session_id,t.session_id FROM task_dispatch d JOIN agent_tasks t ON t.id=d.task_id WHERE t.session_id IS NOT NULL")
            .fetch_all(&pool).await;
        let (rows, owners) = match owners {
            Ok(owners) => (rows, owners.into_iter().collect()),
            Err(error) => {
                eprintln!(
                    "(jeeves) cannot load task permission ownership; execution blocked: {error}"
                );
                (None, HashMap::new())
            }
        };
        Arc::new(Self {
            rows: Mutex::new(rows),
            owners: Mutex::new(owners),
            updates: tokio::sync::Mutex::new(()),
            pool,
        })
    }
    pub(crate) fn bind_task(&self, execution: &str, owner: &str) -> Result<(), String> {
        let owner = self.owner(owner)?;
        if execution == owner {
            return Err("A task cannot own its own permission source".into());
        }
        self.owners
            .lock()
            .map_err(|_| "Permission ownership unavailable")?
            .insert(execution.into(), owner);
        Ok(())
    }
    pub(crate) fn owner(&self, session: &str) -> Result<String, String> {
        let owners = self
            .owners
            .lock()
            .map_err(|_| "Permission ownership unavailable")?;
        let mut current = session;
        for _ in 0..16 {
            match owners.get(current) {
                Some(owner) => current = owner,
                None => return Ok(current.into()),
            }
        }
        Err("Invalid cyclic task permission ownership".into())
    }
    fn snapshot(&self, session_id: &str) -> Result<Option<(PermissionMode, i32)>, String> {
        let session_id = self.owner(session_id)?;
        self.rows
            .lock()
            .map_err(|_| "Permission state unavailable")?
            .as_ref()
            .ok_or_else(|| "Permission state unavailable; restart after fixing storage".to_string())
            .map(|rows| rows.get(&session_id).copied())
    }
    async fn update(&self, params: &SessionPermissionSetParams) -> Result<(), String> {
        let _guard = self.updates.lock().await;
        if self.owner(&params.session_id)? != params.session_id {
            return Err("Change task permissions through its owning conversation".into());
        }
        let current = self.snapshot(&params.session_id)?;
        if current.map_or(0, |(_, revision)| revision) != params.expected_revision {
            return Err("Permission revision changed; reload before saving".into());
        }
        if params.mode == PermissionMode::FullAccess
            && current.map(|(mode, _)| mode) != Some(params.mode)
            && !params.confirm_full_access
        {
            return Err("Full access requires explicit human confirmation".into());
        }
        let mode = serde_json::to_value(params.mode).map_err(|e| e.to_string())?;
        let revision = jeeves_state::session_permissions::save(
            &self.pool,
            &params.session_id,
            mode.as_str().ok_or("Invalid mode")?,
            params.expected_revision,
        )
        .await
        .map_err(|e| e.to_string())?;
        self.rows
            .lock()
            .map_err(|_| "Permission state unavailable")?
            .as_mut()
            .ok_or("Permission state unavailable")?
            .insert(params.session_id.clone(), (params.mode, revision));
        eprintln!(
            "(jeeves) session {} permission changed to {:?} (revision {revision}); in-flight actions are not rolled back",
            params.session_id, params.mode
        );
        Ok(())
    }
}
impl ExecutionPolicy for Policies {
    fn authorize(
        &self,
        ctx: ToolCtx,
        name: String,
        effect: jeeves_tools::ToolEffect,
        args: serde_json::Value,
    ) -> AuthorizationFuture {
        let policy = self.snapshot(&ctx.session_id);
        let handoff = ctx.session_id == crate::interaction::MAIN_SESSION
            && name == crate::interaction::task_tool::NAME;
        let owner_review = ctx.session_id == crate::interaction::MAIN_SESSION
            && name == "recurring_work"
            && effect == jeeves_tools::ToolEffect::Draft;
        Box::pin(async move {
            let Some((mode, revision)) = policy? else {
                return Ok(Authorization {
                    revision: 0,
                    approved: false,
                });
            };
            let read = effect.is_read()
                || proven_read_call(&name, &args)
                || handoff
                || (owner_review
                    && matches!(args["action"].as_str(), Some("list" | "capabilities")));
            if mode == PermissionMode::ReadOnly && !read {
                return Err(format!("Read-only session refuses {name}"));
            }
            if name == "delegate_task" && mode != PermissionMode::FullAccess {
                return Err(
                    "Delegation is unavailable in constrained sessions; run the tools directly"
                        .into(),
                );
            }
            let mut approved = false;
            if mode == PermissionMode::Standard
                && !read
                && !owner_review
                && effect != jeeves_tools::ToolEffect::Send
            {
                let broker = ctx
                    .approval
                    .as_ref()
                    .ok_or("This action needs a human approval channel")?;
                let preview = redact(args).to_string();
                if preview.len() > 8192 {
                    return Err(
                        "Action preview exceeds approval limit; use smaller arguments".into(),
                    );
                }
                let outcome = broker
                    .request(ApprovalRequest {
                        tool: name,
                        args_preview: preview,
                        reason: "Standard session: confirm this operation once".into(),
                        timeout_secs: 120,
                        remember: None,
                        action: None,
                    })
                    .await;
                if !outcome.approved() {
                    return Err(outcome.refusal("action"));
                }
                approved = true;
            }
            Ok(Authorization { revision, approved })
        })
    }
    fn validate(&self, session: &str, revision: i32) -> Result<(), String> {
        if self.snapshot(session)?.map_or(0, |(_, v)| v) != revision {
            return Err(
                "Session permissions changed; this pending action is no longer authorized".into(),
            );
        }
        Ok(())
    }
}

fn invalid(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}
impl Server {
    pub(crate) fn permission_note(&self, session: &str) -> &'static str {
        match self.permissions.snapshot(session) {
            Ok(Some((PermissionMode::ReadOnly, _))) => {
                "\nHuman session policy: read-only. Work in this session or its tasks is read-only. The delegate_task subagent tool is unavailable."
            }
            Ok(Some((PermissionMode::Standard, _))) => {
                "\nHuman session policy: standard. Effects require explicit per-operation human approval. The delegate_task subagent tool is unavailable. Approval is not task completion."
            }
            Ok(Some((PermissionMode::FullAccess, _))) => {
                "\nHuman session policy: full access within enabled capabilities, allowed roots and sandbox constraints. Do not infer broader authority."
            }
            _ => "",
        }
    }
    pub(crate) async fn permission_view(
        &self,
        session: &str,
    ) -> Result<SessionPermissionResponse, ErrorObject> {
        if self.chat.session(session).await.map_err(invalid)?.is_none() {
            return Err(invalid("Session not found"));
        }
        let policy = self.permissions.snapshot(session).map_err(invalid)?;
        let settings = self.settings();
        Ok(SessionPermissionResponse {
            session_id: session.into(),
            mode: policy.map(|(mode, _)| mode),
            revision: policy.map_or(0, |(_, v)| v),
            exec_enabled: settings.exec_enabled,
            write_enabled: settings.write_enabled,
            delegate_enabled: settings.delegate_enabled,
        })
    }
    pub(crate) async fn set_permission(
        &self,
        params: SessionPermissionSetParams,
        sink: &EventSink,
    ) -> Result<SessionPermissionResponse, ErrorObject> {
        self.permissions.update(&params).await.map_err(invalid)?;
        self.browser.pause_owner(&params.session_id).await;
        let turns = self
            .active_turns(None)
            .into_iter()
            .filter(|(_, session, _)| {
                self.permissions
                    .owner(session)
                    .is_ok_and(|owner| owner == params.session_id)
            });
        if let Ok(mut pending) = self.approvals.lock() {
            for (turn_id, _, _) in turns {
                pending.remove(&turn_id);
            }
        }

        let view = self.permission_view(&params.session_id).await?;
        sink.notification(jeeves_protocol::ServerNotification::PermissionChanged(
            view.clone(),
        ));
        Ok(view)
    }
    pub(crate) fn session_approval(
        &self,
        session: &str,
        turn: &str,
        sink: &EventSink,
    ) -> Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>> {
        Some(Arc::new(SessionBroker {
            policies: self.permissions.clone(),
            session: session.into(),
            interactive: self.approval_bridge_forced(turn, sink),
            legacy: self.approval_bridge(turn, sink),
        }))
    }
}

struct SessionBroker {
    policies: Arc<Policies>,
    session: String,
    interactive: Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>>,
    legacy: Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>>,
}
impl jeeves_tools::approval::ApprovalBroker for SessionBroker {
    fn request(&self, request: ApprovalRequest) -> jeeves_tools::approval::ApprovalFut {
        match self.policies.snapshot(&self.session) {
            Ok(Some((PermissionMode::ReadOnly, _))) | Err(_) => {
                Box::pin(async { ApprovalOutcome::Withdrawn })
            }
            Ok(Some((PermissionMode::FullAccess, _))) if request.action.is_some() => {
                match &self.interactive {
                    Some(broker) => broker.request(request),
                    None => Box::pin(async { ApprovalOutcome::Withdrawn }),
                }
            }
            Ok(Some((PermissionMode::Standard, _))) => match &self.interactive {
                Some(broker) => broker.request(request),
                None => Box::pin(async { ApprovalOutcome::Withdrawn }),
            },
            Ok(Some((PermissionMode::FullAccess, _))) if self.legacy.is_some() => {
                Box::pin(async { ApprovalOutcome::Approved })
            }
            _ => match &self.legacy {
                Some(broker) => broker.request(request),
                None => Box::pin(async { ApprovalOutcome::Withdrawn }),
            },
        }
    }
}
