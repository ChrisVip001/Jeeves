use crate::{
    builtin_tools, lock, memory_worker, models, resources, subagent, BuiltinTools, EventSink,
    Server,
};
use anyhow::Result;
use jeeves_config::Settings;
use jeeves_state::{Chat, Episodes, Facts};
use sqlx::sqlite::SqlitePool;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Mutex as AsyncMutex;

impl Server {
    /// Start the background memory worker on this server (idempotent).
    ///
    /// **The process owns the worker, not the library**: the CLI (REPL and the stdio
    /// daemon) calls this; `open()` does not, so the eval harness and assembly tests stay
    /// deterministic (a background model call must never race an assertion). Without a
    /// worker, `finalize_turn` does the same memory work inline — one function, two callers.
    pub fn attach_memory_worker(&mut self) {
        if self.memory_worker.is_some() {
            return;
        }
        let worker = memory_worker::MemoryWorker::spawn(memory_worker::Deps {
            pool: self.pool.clone(),
            chat: self.chat.clone(),
            facts: self.facts.clone(),
            episodes: self.episodes.clone(),
            settings: self.settings.clone(),
            resolved: self.resolved.clone(),
            _execution_owner: self._execution_owner.clone(),
        });
        self.memory_worker = Some(worker);
    }

    /// Stop the worker, waiting briefly for a run in flight. Called on orderly shutdown.
    pub async fn stop_memory_worker(&self) {
        if let Some(worker) = self.memory_worker.as_ref() {
            worker.stop().await;
        }
    }

    /// Isolated embedding: the caller must exclusively own the supplied store.
    /// Production entry points use `open` to acquire ownership before recovery.
    pub async fn boot(pool: SqlitePool, settings: Settings) -> Self {
        Self::boot_owned(pool, settings, None).await
    }

    async fn boot_owned(
        pool: SqlitePool,
        mut settings: Settings,
        execution_owner: Option<Arc<lock::DaemonLock>>,
    ) -> Self {
        if let Err(error) = sqlx::query(
            "UPDATE model_requests SET termination='interrupted' WHERE termination='in_flight'",
        )
        .execute(&pool)
        .await
        {
            eprintln!("(jeeves) interrupted model requests could not be closed: {error}");
        }
        let secrets = models::new_secrets();
        if let Err(error) = crate::browser::downloads::recover(&pool, &settings.home).await {
            eprintln!("(jeeves) browser artifact recovery failed: {error}");
        }
        let boot_settings = Arc::new(settings.clone());
        let file = match jeeves_config::models::read_models(&settings.home) {
            Ok(mut file) => {
                if let Err(e) = jeeves_config::models::recover_change(
                    &settings.home,
                    &mut file,
                    &mut secrets.lock().expect("secrets lock"),
                ) {
                    eprintln!("(jeeves) model change recovery pending: {e}");
                }
                file
            }
            Err(e) => {
                eprintln!("(jeeves) model profiles unavailable: {e}");
                Default::default()
            }
        };
        jeeves_config::models::apply_active(
            &file,
            &boot_settings.home,
            &mut *secrets.lock().expect("secrets lock"),
            &mut settings,
        );
        let models = models::new_models(file);
        let routes_boot = jeeves_config::routes::load_routes(&settings.home);
        if routes_boot.mode == jeeves_config::routes::RouteMode::Auto {
            eprintln!(
                "(jeeves) model routing: auto ({} profiles, {} tier mappings)",
                jeeves_config::routes::routes_path(&settings.home).display(),
                routes_boot.routes.len()
            );
        }
        let route_cache = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let settings = Arc::new(RwLock::new(settings));
        let snapshot = settings
            .read()
            .expect("the settings lock should not be poisoned")
            .clone();
        let catalog = Arc::new(jeeves_provider::catalog::Catalog::load(&snapshot.home));
        let resolved = match jeeves_provider::resolve_with(&snapshot, &catalog) {
            Ok(resolved) => Some(resolved),
            Err(reason) => {
                eprintln!("(jeeves) no model configured yet, turn/* unavailable — {reason}");
                None
            }
        };
        let resolved = Arc::new(RwLock::new(resolved));
        jeeves_memory::skills::seed_default_skills(&snapshot.home);
        {
            let mut loader = jeeves_memory::skills::SkillLoader::new(
                jeeves_memory::skills::SkillLoader::dirs_for(&snapshot.home),
            );
            for (skill, missing) in loader.dangling_dependencies() {
                eprintln!("(jeeves) skill '{skill}' depends on '{missing}', which is missing; it takes no part in implicit triggering");
            }
        }
        if std::fs::read_dir(snapshot.home.join("tools"))
            .map(|dir| {
                dir.flatten()
                    .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            })
            .unwrap_or(false)
        {
            eprintln!(
                "(jeeves) {} holds tool definitions, but custom tools (`define_tool`) were removed — turn each into a skill that runs the command with `run_command`",
                snapshot.home.join("tools").display()
            );
        }

        let notifier = jeeves_tools::notify::load(&snapshot.home, snapshot.notify_enabled);
        let approval_log = jeeves_state::Approvals::new(pool.clone());
        let grants = jeeves_state::Grants::new(pool.clone());
        let restored_approvals = match grants.pending_restore(chrono::Utc::now().timestamp()).await
        {
            Ok(rows) => {
                if !rows.is_empty() {
                    eprintln!(
                        "(jeeves) {} approval question(s) outlived the restart and wait for an answer",
                        rows.len()
                    );
                }
                rows
            }
            Err(error) => {
                eprintln!("(jeeves) pending approvals could not be restored: {error}");
                Vec::new()
            }
        };
        let mut boot = snapshot.clone();
        boot.exec_enabled = true;
        boot.write_enabled = true;
        boot.delegate_enabled = true;
        let builtin = builtin_tools(
            &boot,
            pool.clone(),
            notifier.clone(),
            execution_owner.clone(),
        )
        .await;
        let boot_home = snapshot.home.clone();
        let BuiltinTools {
            browser,
            mut tools,
            jobs: jobs_handle,
            exec_allow,
            exec_session_allow,
            write_approved_dirs,
            mcp,
        } = builtin;
        let mcp = Arc::new(crate::server::mcp_live::McpLive::new(
            mcp, &boot_home, &mut tools,
        ));
        let permissions = crate::permissions::Policies::load(pool.clone()).await;
        tools.set_execution_policy(permissions.clone());
        tools.set_resources(Arc::new(resources::Mutations::default()));
        let facts = Facts::new(pool.clone());
        let episodes = Episodes::new(pool.clone());
        let chat = Chat::new(pool.clone());
        match chat.recover_consolidation_claims().await {
            Ok(count) if count > 0 => eprintln!("(jeeves) recovered {count} unfinished memory source rows; extraction will retry without replaying task tools"),
            Ok(_) => {}
            Err(error) => eprintln!("(jeeves) memory claim recovery failed; claims retained for the next startup: {error}"),
        }
        let ledger = jeeves_state::Ledger::new(pool.clone());
        if let Ok(0) = ledger.count().await {
            match jeeves_state::ledger::backfill_from_chat(&pool).await {
                Ok(report) if report.turns > 0 => eprintln!(
                    "(jeeves) first start: backfilling {} turns from {} sessions into the ledger (marked folded — the tool output of those turns survives only as a summary)",
                    report.sessions, report.turns
                ),
                Ok(_) => {}
                Err(e) => eprintln!("(jeeves) ledger backfill failed (not blocking startup; run `jeeves memory backfill` later): {e}"),
            }
        }
        let usage = jeeves_state::Usage::new(pool.clone());
        let checkpoints = jeeves_state::Checkpoints::new(pool.clone());

        let todo = Arc::new(jeeves_tools::todo::TodoBoard::new());
        tools.register(jeeves_tools::todo::todo_write(todo.clone()));

        {
            tools.register_exclusive(jeeves_tools::subagent::delegate_task(Arc::new(
                subagent::Delegated {
                    tools: Arc::new(tools.clone()),
                    mcp: mcp.clone(),
                    facts: facts.clone(),
                    episodes: episodes.clone(),
                    chat: chat.clone(),
                    settings: settings.clone(),
                    resolved: resolved.clone(),
                    catalog: catalog.clone(),
                },
            )));
        }

        let server = Self {
            boot_settings,
            browser,
            permissions,
            facts,
            episodes,
            chat,
            ledger,
            memory_worker: None,
            usage,
            checkpoints,
            notifier,
            approval_log,
            settings,
            resolved,
            tools: Arc::new(tools),
            turns: Arc::new(Mutex::new(HashMap::new())),
            approvals: Arc::new(Mutex::new(HashMap::new())),
            exec_allow,
            exec_session_allow,
            write_approved_dirs,
            mcp,
            todo,
            jobs: jobs_handle,
            queue: jeeves_state::Queue::new(pool.clone()),
            tasks: jeeves_state::Tasks::new(pool.clone()),
            completions: Arc::new(jeeves_state::completions::Completions::new(pool.clone())),
            completion_lock: Arc::new(AsyncMutex::new(())),
            runs: jeeves_state::Runs::new(pool.clone()),
            steps: jeeves_state::Steps::new(pool.clone()),
            events: jeeves_state::Events::new(pool.clone()),
            artifacts: jeeves_state::Artifacts::new(pool.clone()),
            suspending: Arc::new(Mutex::new(HashSet::new())),
            models,
            route_cache,
            catalog,
            session_locks: Arc::new(Mutex::new(HashMap::new())),
            secrets,
            pool,
            _execution_owner: execution_owner,
            context_usage: Arc::new(Mutex::new(HashMap::new())),
            activations: Arc::new(Mutex::new(HashMap::new())),
            capabilities: Default::default(),
            grants,
            restored_approvals: Arc::new(Mutex::new(restored_approvals)),
            resume_yes: Default::default(),
        };
        server.restore_tasks().await;
        crate::completion_delivery::start(
            &server.completions,
            server.notifier.as_ref(),
            &server.completion_lock,
            server.settings().home,
        );
        server
    }

    /// Flush already committed production receipts; this grants no execution capability.
    pub async fn flush_completion_notices(&self) {
        crate::completion_delivery::flush(self).await;
    }

    /// Boot restore for long tasks (migrations 0014 + 0017), in dependency order:
    ///
    /// 1. a step left `running` means "sent, its result never landed". It becomes `waiting` —
    ///    **never replayed**: a write that may already have happened is a human's call;
    /// 2. a run left `running` belonged to the dead process: closed as `waiting` too;
    /// 3. a `running` task row is marked suspended with the honest reason (the turn's `open`
    ///    checkpoint may then flip it back to running).
    ///
    /// There is no goal to restore any more: a goal is a column on the row, so step 3 already
    /// brings it back with everything else.
    pub(crate) async fn restore_tasks(&self) {
        if self.jobs.is_none() {
            match jeeves_state::Jobs::new(self.pool.clone())
                .reconcile_interrupted()
                .await
            {
                Ok(jobs) => {
                    for job in jobs {
                        eprintln!(
                            "(jeeves) job {} interrupted at restart; execution host is disabled",
                            job.id
                        );
                    }
                }
                Err(e) => eprintln!("(jeeves) disabled job host reconciliation failed: {e}"),
            }
        }

        match self
            .steps
            .settle_running("the process exited before this step was recorded, so the outcome is unknown — confirm whether to redo it")
            .await
        {
            Ok(0) => {}
            Ok(count) => eprintln!(
                "(jeeves) reconcile: {count} step(s) were interrupted before their result was recorded; marked waiting (not replayed — waiting on you)"
            ),
            Err(e) => eprintln!("(jeeves) step reconciliation could not read the database (not blocking startup): {e}"),
        }
        match self.runs.open_runs().await {
            Ok(runs) => {
                for run in runs {
                    match self.runs.close(&run.id, jeeves_state::RunStatus::Waiting,
                        Some("the process restarted before this run finished; waiting for confirmation")).await {
                        Ok(()) => eprintln!("(jeeves) reconcile: run {} (task {}) marked waiting after restart", run.id, run.task_id),
                        Err(error) => eprintln!("(jeeves) reconcile: could not mark run {} waiting: {error}", run.id),
                    }

                }
            }
            Err(e) => eprintln!("(jeeves) run-ledger reconciliation could not read the database (not blocking startup): {e}"),
        }
        match self.tasks.reconcile_restart().await {
            Ok(rows) if !rows.is_empty() => {
                for row in &rows {
                    eprintln!(
                        "(jeeves) reconcile: task \"{}\" was still running when the process exited; marked suspended (session {})",
                        row.title,
                        row.session_id.as_deref().unwrap_or("(no session)")
                    );
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!(
                "(jeeves) task reconciliation could not read the database (not blocking startup): {e}"
            ),
        }
        let dispatch = jeeves_state::dispatch::TaskDispatch::new(self.pool.clone());
        match dispatch.reconcile().await {
            Ok(ids) => {
                for id in ids {
                    eprintln!(
                        "(jeeves) task {id}: dispatch interrupted before checkpoint; needs review"
                    );
                }
            }
            Err(e) => eprintln!("(jeeves) task dispatch reconciliation failed: {e}"),
        }
        match self.queue.reconcile_interrupted().await {
            Ok(rows) => {
                for row in rows {
                    let note = "(this queued instruction was interrupted; its outcome is unknown. Review the transcript and confirm before retrying.)";
                    eprintln!(
                        "(jeeves) queued instruction {} in session {} needs attention after restart",
                        row.id, row.session_id
                    );
                    if let Err(e) = self
                        .chat
                        .append_exchange_for(
                            &row.message,
                            note,
                            &row.session_id,
                            "app-server",
                            None,
                            None,
                        )
                        .await
                    {
                        eprintln!("(jeeves) interrupted queue notice could not be recorded: {e}");
                    }
                }
            }
            Err(e) => eprintln!("(jeeves) queued instruction reconciliation failed: {e}"),
        }
        match jeeves_state::waits::JobWaits::new(self.pool.clone())
            .pending()
            .await
        {
            Ok(waits) => {
                let (sink, receiver) = EventSink::channel();
                drop(receiver);
                for wait in waits {
                    eprintln!(
                        "(jeeves) restored background-job wait for task {}",
                        wait.task_id
                    );
                    crate::task::waits::arm(self.clone(), sink.clone(), wait);
                }
            }
            Err(e) => eprintln!("(jeeves) durable job waits could not be restored: {e}"),
        }
        match jeeves_state::external_waits::ExternalWaits::new(self.pool.clone())
            .pending()
            .await
        {
            Ok(waits) => {
                let (sink, receiver) = EventSink::channel();
                drop(receiver);
                for wait in waits {
                    eprintln!(
                        "(jeeves) restored external-input wait for task {}",
                        wait.task_id
                    );
                    crate::input_wake::arm(self.clone(), sink.clone(), wait);
                }
            }
            Err(error) => eprintln!("(jeeves) external-input waits could not be restored: {error}"),
        }
        match self.queue.sessions_with_pending().await {
            Ok(sessions) => {
                for session in sessions {
                    eprintln!("(jeeves) reconcile: session {session} still has queued messages; it carries on");
                    let server = self.clone();
                    tokio::spawn(async move { crate::turn::drain_queue(&server, &session).await });
                }
            }
            Err(e) => {
                eprintln!("(jeeves) queued messages could not be read (not blocking startup): {e}")
            }
        }
    }
}

/// Open state.db under `settings.home` and assemble.
pub async fn open(settings: &Settings) -> Result<Server> {
    let mut settings = settings.clone();
    let saved = jeeves_config::load_patch(&settings.home);
    jeeves_config::apply_patch(&saved, &mut settings);

    if let Err(why) = settings.validate() {
        anyhow::bail!(
            "The configuration has a problem, so Jeeves will not start: {why}\n( fix that variable, or drop the override in <home>/settings.json; the boundary table is in jeeves-config's BOUNDS and docs/configuration.md)"
        );
    }

    settings.ensure_home()?;
    let ownership = Arc::new(lock::acquire(&settings.home).map_err(anyhow::Error::msg)?);
    let pool = jeeves_state::open(&settings.home.join("state.db")).await?;
    Ok(Server::boot_owned(pool, settings, Some(ownership)).await)
}
