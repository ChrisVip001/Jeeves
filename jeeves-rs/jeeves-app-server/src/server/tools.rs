use jeeves_config::Settings;
use jeeves_state::{Chat, Episodes, Facts};
use jeeves_tools::ToolCtx;
use jeeves_tools::ToolRegistry;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{jobs, lock, Server};

impl Server {
    /// The environment handed to handlers when executing tools. **Constructed in exactly one place**
    /// (shared by turn and subagent) — adding a field can never miss a path.
    ///
    /// `approval` is this turn's approval channel: the subagent passes `None` (it must not block on a human).
    pub(crate) fn tool_ctx(
        &self,
        session_id: &str,
        cwd: Option<std::path::PathBuf>,
        approval: Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>>,
    ) -> ToolCtx {
        let settings = self.settings();
        tool_ctx(
            &self.facts,
            &self.episodes,
            &self.chat,
            &settings.home,
            Injections {
                session_id: session_id.to_string(),
                cwd,
                approval,
                jobs: self.jobs.clone(),
            },
        )
    }
}

/// The tool names each capability switch controls. `job` goes with `run_command`.
const EXEC_TOOLS: [&str; 2] = ["run_command", "job"];
const WRITE_TOOLS: [&str; 3] = ["write_file", "edit", "apply_patch"];

/// The table with every capability the settings leave off taken away. Cheap (tools are `Arc`s),
/// so a turn takes it fresh: the switches are read at turn start, never at boot.
pub(crate) fn enabled_tools(all: &ToolRegistry, settings: &Settings) -> ToolRegistry {
    let mut tools = all.clone();
    let off = [
        (settings.exec_enabled, &EXEC_TOOLS[..]),
        (settings.write_enabled, &WRITE_TOOLS[..]),
        (
            settings.delegate_enabled,
            &[jeeves_tools::subagent::NAME][..],
        ),
    ];
    for (enabled, names) in off {
        if !enabled {
            for name in names {
                tools = tools.without(name);
            }
        }
    }
    tools
}

/// The built-in tool table, plus the two live cells the approval answer writes into.
///
/// Bundled instead of growing the return tuple: every new shared cell would otherwise add a
/// positional element at every call site, and a swapped pair of `Arc`s compiles happily.
pub(crate) struct BuiltinTools {
    pub(crate) browser: Arc<crate::browser::BrowserHost>,
    pub(crate) tools: ToolRegistry,
    pub(crate) jobs: Option<Arc<dyn jeeves_tools::jobs::JobRegistry>>,
    /// See `Server::exec_allow` — same `Arc` the exec policy holds.
    pub(crate) exec_allow: Arc<Mutex<Vec<String>>>,
    /// See `Server::exec_session_allow` — same `Arc` the exec policy holds.
    pub(crate) exec_session_allow: Arc<Mutex<Vec<String>>>,
    /// See `Server::write_approved_dirs` — same `Arc` the write policy holds.
    pub(crate) write_approved_dirs: Arc<Mutex<Vec<PathBuf>>>,
    /// The connected MCP servers, kept so an orderly shutdown can end their sessions.
    pub(crate) mcp: jeeves_mcp::McpClient,
}

/// Built-in tools, plus MCP tools configured in `<home>/mcp.json`.
///
/// An unreachable MCP server only leaves a warning on stderr and Jeeves starts as usual —
/// a misconfigured server must not keep the whole assistant from starting (the same rule as
/// "a tool execution failure must not crash a turn"). Managed search connects lazily on a web
/// call, independently of mcp.json. Built-in tool table + background job
/// table (the job table is built alongside `run_command`; both share one exec policy, same return place).
/// The execution policy, built in one place.
///
/// The `session_allow` cell is shared, so an "always allow in this session" answered for
/// `run_command` is visible to the very next call.
/// The vault's `(NAME, value)` pairs, read at the moment a command starts or its output is scrubbed,
/// so a secret added in Settings works on the very next command.
fn vault_values(home: PathBuf) -> jeeves_tools::exec::SecretsFn {
    jeeves_tools::exec::SecretsFn(Arc::new(move || {
        let names = jeeves_config::vault::names(&home);
        if names.is_empty() {
            return Vec::new();
        }
        let mut store = jeeves_config::models::SecretStore::auto();
        names
            .into_iter()
            .filter_map(|name| {
                let value = jeeves_config::vault::get(&mut store, &home, &name)?;
                Some((name, value))
            })
            .collect()
    }))
}

pub(crate) fn exec_policy(
    settings: &Settings,
    allow: Arc<Mutex<Vec<String>>>,
    session_allow: Arc<Mutex<Vec<String>>>,
) -> jeeves_tools::exec::ExecPolicy {
    jeeves_tools::exec::ExecPolicy {
        allow,
        timeout_secs: settings.exec_timeout_secs,
        network: settings.exec_network,
        approval: settings.approval == "on-request",
        approval_timeout_secs: settings.approval_timeout_secs,
        worktree: None,
        spill_dir: Some(settings.home.join("spill")),
        extra_roots: settings.exec_writable_roots.clone(),
        home: settings.home.clone(),
        session_allow,
        secrets: Some(vault_values(settings.home.clone())),
    }
}

pub(crate) async fn builtin_tools(
    settings: &Settings,
    pool: sqlx::SqlitePool,
    notifier: Option<Arc<dyn jeeves_tools::notify::Notifier>>,
    execution_owner: Option<Arc<lock::DaemonLock>>,
) -> BuiltinTools {
    let browser = crate::browser::BrowserHost::new(pool.clone(), settings.browser.clone()).await;
    let primary = super::search_mcp::handler(&settings.home);
    let search_enabled = primary.is_some();
    let mut tools = jeeves_tools::handlers::build_default_with_web(primary);
    if browser.available() {
        let host: Arc<dyn jeeves_tools::browser::BrowserHost> = browser.clone();
        tools.register_exclusive(jeeves_tools::browser::browser_observe(host.clone()));
        tools.register_exclusive(jeeves_tools::browser::browser_act(host));
        eprintln!("(jeeves) browser components ready; a human must open each task browser and select its website scope");
    }
    let exec_session_allow: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let exec_allow: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(settings.exec_allow.clone()));
    let write_approved_dirs: Arc<Mutex<Vec<PathBuf>>> =
        Arc::new(Mutex::new(settings.write_allow_dirs.clone()));
    if !search_enabled && !jeeves_tools::web::search_ready() {
        eprintln!(
            "(jeeves) search not ready: no search key configured and the keyless backends are off (JEEVES_SEARCH_DDG=0, JEEVES_SEARCH_BING=0); search_web was not registered"
        );
    }
    let mut jobs_handle: Option<Arc<dyn jeeves_tools::jobs::JobRegistry>> = None;
    if settings.exec_enabled {
        let sandbox = if jeeves_tools::exec::sandbox_available() {
            "available"
        } else {
            "unavailable (commands will be refused)"
        };
        let allow = if settings.exec_allow.is_empty() {
            "empty (nothing is allowed)".to_string()
        } else {
            settings.exec_allow.join(", ")
        };
        let network = if settings.exec_network {
            "network allowed"
        } else {
            "network cut inside the sandbox (JEEVES_EXEC_NETWORK=1 enables it)"
        };
        let extra = if settings.exec_writable_roots.is_empty() {
            String::new()
        } else {
            format!(
                ", extra writable roots: {}",
                settings
                    .exec_writable_roots
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let approval = if settings.approval == "on-request" {
            format!(
                ", asks you when nothing matches ({}s without an answer counts as a denial)",
                settings.approval_timeout_secs
            )
        } else {
            ", refuses outright when nothing matches (JEEVES_APPROVAL=on-request asks instead)"
                .to_string()
        };
        eprintln!(
            "(jeeves) exec tool enabled: sandbox {sandbox}, {network}, allow rules: {allow}{approval}{extra}"
        );
        let dropped =
            jeeves_tools::roots::resolve_roots(&[], &settings.exec_writable_roots, &settings.home)
                .dropped;
        if !dropped.is_empty() {
            eprintln!(
                "(jeeves) these extra writable roots were refused (they would hand over the control plane: home itself, an ancestor of home, or an entry in HOME_CONTROL): {}",
                dropped
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let policy = exec_policy(settings, exec_allow.clone(), exec_session_allow.clone());
        tools.register_exclusive(jeeves_tools::exec::run_command(policy.clone()));
        let ledger = jeeves_state::Jobs::new(pool.clone());
        let jobs_impl = jobs::Jobs::new(policy, settings.home.clone(), ledger)
            .with_notifier(notifier.clone())
            .with_execution_owner(execution_owner);
        jobs_impl.reconcile().await;
        let jobs: Arc<dyn jeeves_tools::jobs::JobRegistry> = Arc::new(jobs_impl);
        tools.register(jeeves_tools::jobs::job(Some(jobs.clone())));
        jobs_handle = Some(jobs);
        eprintln!(
            "(jeeves) background jobs available: run_command with background=true, plus the `job` tool (output / list / kill)"
        );
    }
    if settings.write_enabled {
        let policy = jeeves_tools::fs_write::WritePolicy {
            approval: settings.approval == "on-request",
            approval_timeout_secs: settings.approval_timeout_secs,
            extra_roots: settings.exec_writable_roots.clone(),
            approved_dirs: write_approved_dirs.clone(),
        };
        tools.register_exclusive(jeeves_tools::fs_write::write_file(policy.clone()));
        tools.register_exclusive(jeeves_tools::fs_write::edit(policy.clone()));
        tools.register_exclusive(jeeves_tools::patch::apply_patch(policy));
        eprintln!(
            "(jeeves) file tools enabled: write_file / edit / apply_patch (coding tasks write only their worktree; ordinary turns use their working directory + extra roots; with no directory picked nothing can be written; on-request mode asks outside previously approved directories)"
        );
    }
    match jeeves_connector::config::load(&settings.home) {
        Ok(config) => {
            if let Some(calendar) = config.calendar.clone() {
                crate::connector_settings::register_native(
                    &mut tools,
                    "calendar",
                    jeeves_connector::calendar::calendar_list_tool(calendar),
                );
                eprintln!("(jeeves) connector calendar enabled: calendar_list — read-only");
            }
            if let Some(email) = config.email.clone() {
                crate::connector_settings::register_native(
                    &mut tools,
                    "email",
                    jeeves_connector::email::email_read_tool(email.clone()),
                );
                if email.allow_recipients.is_empty() {
                    eprintln!(
                        "(jeeves) connector email is configured but its allowlist is empty — email_read is enabled; email_send was not registered"
                    );
                } else {
                    crate::connector_settings::register_native(
                        &mut tools,
                        "email",
                        jeeves_connector::email::email_send_tool(email.clone()),
                    );
                    eprintln!(
                        "(jeeves) connector email enabled: email_read + email_send — {}",
                        if email.live {
                            "live (each recipient and content is approved separately)"
                        } else {
                            "draft mode (writes to outbox only, never sends)"
                        }
                    );
                }
            }
            if let Some(feishu) = config.feishu.clone() {
                if feishu.identity() == Ok("user") {
                    jeeves_connector::feishu_oauth::spawn_renewer(
                        feishu.clone(),
                        settings.home.clone(),
                    );
                    eprintln!("(jeeves) Feishu sends as the user: the user token renews itself in the background");
                }
                if feishu.allow_recipients.is_empty() {
                    eprintln!(
                        "(jeeves) connector feishu is configured but its allowlist is empty (nobody can receive) — feishu_send was not registered; fill in allow_recipients in {}",
                        settings.home.join("connectors.json").display()
                    );
                } else {
                    let mode = if feishu.live {
                        "live (every send asks a person first)"
                    } else {
                        "draft mode (writes to outbox only, never sends)"
                    };
                    crate::connector_settings::register_native(
                        &mut tools,
                        "feishu",
                        jeeves_connector::feishu_send_tool(feishu),
                    );
                    eprintln!("(jeeves) connector feishu enabled: feishu_send — {mode}");
                }
            }
        }
        Err(e) => eprintln!(
            "(jeeves) the connector configuration has a problem; no connectors registered: {e}"
        ),
    }
    jeeves_tools::exec::prune_spill(&settings.home, 7);

    let mcp = jeeves_mcp::McpClient::connect_home(&settings.home).await;
    for warning in mcp.warnings() {
        eprintln!("(jeeves) {warning}");
    }
    let servers = mcp.servers();
    let count = mcp.tools().len();
    if !servers.is_empty() {
        eprintln!(
            "(jeeves) MCP: connected {} servers ({count} tools): {}",
            servers.len(),
            servers.join(", ")
        );
    }
    BuiltinTools {
        browser,
        tools,
        jobs: jobs_handle,
        exec_allow,
        exec_session_allow,
        write_approved_dirs,
        mcp,
    }
}

/// The **only** construction site of the tool execution environment. Both turn and subagent go through it —
/// writing it twice is just waiting for the day a field is added and one site is missed (a subagent with
/// half a ctx is very hard to debug).
/// The in-turn tool-result budget (`JEEVES_TOOL_RESULT_*`). Turn and subagent share one policy — the subagent inherits the parent's discipline.
pub(crate) fn tool_result_budget(settings: &Settings) -> jeeves_loop::budget::ToolResultBudget {
    jeeves_loop::budget::ToolResultBudget {
        total_chars: settings.tool_result_total_chars.max(0) as usize,
        per_result_chars: settings.tool_result_max_chars.max(0) as usize,
        keep_recent: settings.tool_result_keep_recent.max(0) as usize,
    }
}

/// What gets injected into the handler for this turn: session name, approval channel.
///
/// Bundled into a struct instead of adding more parameters: they **always change together** (each new
/// injected handle would add a line at four call sites), and clippy stops at the 8th parameter anyway.
#[derive(Clone, Default)]
pub(crate) struct Injections {
    pub session_id: String,
    /// This turn's working directory; None = the process cwd.
    pub cwd: Option<std::path::PathBuf>,
    pub approval: Option<Arc<dyn jeeves_tools::approval::ApprovalBroker>>,
    pub jobs: Option<Arc<dyn jeeves_tools::jobs::JobRegistry>>,
}

pub(crate) fn tool_ctx(
    facts: &Facts,
    episodes: &Episodes,
    chat: &Chat,
    home: &std::path::Path,
    injected: Injections,
) -> ToolCtx {
    ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        activate: None,
        facts: facts.clone(),
        episodes: episodes.clone(),
        chat: chat.clone(),
        home: home.to_path_buf(),
        session_id: injected.session_id,
        cwd: injected.cwd,
        approval: injected.approval,
        turn_limits: std::sync::Arc::new(jeeves_tools::web::TurnLimits::from_env()),
        jobs: injected.jobs,
        on_delta: None,
    }
}
