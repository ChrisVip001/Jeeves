//! `jeeves-ops` — the private Runtime HTTP API used by the Go gateway and channel adapters.
//!
//! It is a **client** of `jeeves app-server`, like the CLI, the TS gateway and the Python SDK.
//! The only special thing: it translates the protocol into what a browser understands:
//!
//! ```text
//!   GET  /api/data     the four read methods → one screen of data (DashboardData)
//!   GET  /api/session  session/messages → what one session said (one page)
//!   POST /api/interaction     interaction/start → one SSE stream (JSON of ServerNotification)
//! ```
//!
//! The dashboard reads runtime-owned state through the protocol. It must not create a
//! competing execution owner or run recovery. Every client observes the same runtime.

mod api;
mod capabilities;
mod client;
mod discovery;
mod gateways;
mod lifecycle;
mod memory_api;
mod webhooks;

#[cfg(test)]
#[path = "capabilities_tests.rs"]
mod capabilities_tests;

#[cfg(test)]
#[path = "events_tests.rs"]
mod events_tests;

#[cfg(test)]
#[path = "router_tests.rs"]
mod router_tests;

#[cfg(test)]
#[path = "webhook_tests.rs"]
mod webhook_tests;

use std::future::IntoFuture;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::http::{header, HeaderValue};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use jeeves_config::Settings;
use jeeves_protocol::{ErrorNotification, ErrorObject, ServerNotification};
use tokio::net::TcpListener;
use tower_http::set_header::SetResponseHeaderLayer;

pub use client::AppServer;

/// Default private Runtime port.
pub const DEFAULT_PORT: u16 = 7777;
/// How many ports to scan forward when the default port is taken.
const PORT_SCAN: u16 = 10;

const STARTUP_GRACE: std::time::Duration = std::time::Duration::from_secs(60);
const EARLY_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Ctrl-C, or SIGTERM (what a container runtime sends before it kills the process).
async fn stop_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut term) => {
                term.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

/// Start the private HTTP API, until Ctrl-C or SIGTERM; then drain.
pub async fn serve(settings: Settings) -> Result<()> {
    let token = match settings.ops_token {
        Some(token) => Some(token),
        None => match std::fs::read_to_string(settings.home.join("runtime-token")) {
            Ok(token) => Some(token.trim().to_owned()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error).context("reading the private Runtime credential"),
        },
    }
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| {
        anyhow::anyhow!(
            "set JEEVES_OPS_TOKEN or create a private runtime-token file in JEEVES_HOME"
        )
    })?;

    if !settings.home.exists() {
        eprintln!(
            "(jeeves) the state directory {} does not exist yet; the app-server will create it",
            settings.home.display()
        );
    }

    let first = port();
    let bind_host = bind_addr(&settings.ops_host);
    if first == 0 && !matches!(settings.ops_host.as_str(), "127.0.0.1" | "localhost") {
        anyhow::bail!("JEEVES_PORT=0 requires the loopback Runtime host 127.0.0.1");
    }
    let last = if first == 0 {
        0
    } else {
        first.saturating_add(PORT_SCAN - 1)
    };
    let app = Arc::new(AppServer::spawn(&settings.home, true)?);
    let signal = stop_signal();
    tokio::pin!(signal);
    eprintln!(
        "(jeeves) waiting for Runtime readiness (up to {}s)",
        STARTUP_GRACE.as_secs()
    );
    let ready = tokio::select! {
        biased;
        _=&mut signal=>None,
        result=tokio::time::timeout(STARTUP_GRACE,app.probe())=>Some(result),
    };
    match ready {
        Some(Ok(Ok(_))) => {}
        None => {
            app.shutdown(EARLY_EXIT_GRACE).await;
            return Ok(());
        }
        Some(Ok(Err(error))) => {
            app.shutdown(EARLY_EXIT_GRACE).await;
            anyhow::bail!("Jeeves Runtime did not become ready: {}", error.message);
        }
        Some(Err(_)) => {
            app.shutdown(EARLY_EXIT_GRACE).await;
            anyhow::bail!(
                "Jeeves Runtime startup timed out after {}s",
                STARTUP_GRACE.as_secs()
            );
        }
    }
    let gate = Gate::single(Arc::clone(&app), token);
    let gateways = Arc::clone(&gate.gateways);

    for candidate in first..=last {
        match TcpListener::bind((bind_host, candidate)).await {
            Ok(listener) => {
                if app.is_stopping() {
                    app.shutdown(EARLY_EXIT_GRACE).await;
                    anyhow::bail!("Jeeves Runtime exited during cockpit startup");
                }
                let address = listener.local_addr()?;
                let router = if first == 0 {
                    match discovery::publish(
                        router(gate.clone()),
                        &settings.home,
                        address.port(),
                        &gate.token,
                    ) {
                        Ok(router) => router,
                        Err(error) => {
                            app.shutdown(EARLY_EXIT_GRACE).await;
                            return Err(error);
                        }
                    }
                } else {
                    router(gate)
                };
                println!("Jeeves Runtime → http://{address}");
                gateways.set_url(format!("http://127.0.0.1:{}", address.port()));
                gateways.autostart().await;
                let served = axum::serve(listener, router)
                    .with_graceful_shutdown(app.stopping())
                    .into_future();
                tokio::pin!(served);
                let (http_finished, failure) = tokio::select! {
                    biased;
                    _=&mut signal => (false,None),
                    _=app.stopping() => (false,Some(anyhow::anyhow!("the app-server exited; stopping the cockpit"))),
                    result=&mut served => (true,result.err().map(anyhow::Error::from)),
                };
                lifecycle::drain(&app, &gateways, (!http_finished).then_some(&mut served)).await;
                if let Some(error) = failure {
                    return Err(error);
                }
                return Ok(());
            }
            Err(error) => println!("port {candidate} is unusable ({error}); trying the next one"),
        }
    }

    app.shutdown(EARLY_EXIT_GRACE).await;
    anyhow::bail!("{first}–{last} are all taken on {bind_host}. Pick a free port with JEEVES_PORT.")
}

/// Request gate: the service token and the one app-server.
#[derive(Clone)]
pub struct Gate {
    token: String,
    app: Arc<AppServer>,
    /// Inbound-hook replay and rate state, scoped to this Runtime.
    admissions: Arc<std::sync::Mutex<webhooks::Admissions>>,
    /// The chat gateways this Runtime hosts.
    gateways: Arc<gateways::Gateways>,
}

impl Gate {
    /// One app-server and its service token.
    pub fn single(app: Arc<AppServer>, token: String) -> Self {
        Self {
            gateways: gateways::Gateways::new(app.home(), Some(token.clone())),
            token,
            app,
            admissions: Arc::default(),
        }
    }

    /// The app-server. Unauthenticated routes (inbound webhooks) need it too: their proof is
    /// the hook's own secret, not the dashboard token.
    pub(crate) fn app(&self) -> Arc<AppServer> {
        Arc::clone(&self.app)
    }

    pub(crate) fn admissions(&self) -> &std::sync::Mutex<webhooks::Admissions> {
        &self.admissions
    }
}

async fn authorize(
    axum::extract::State(gate): axum::extract::State<Gate>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !provided.is_some_and(|t| constant_time_eq(t.as_bytes(), gate.token.as_bytes())) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    req.extensions_mut().insert(gate.app());
    req.extensions_mut().insert(Arc::clone(&gate.gateways));
    next.run(req).await
}

/// IP to bind: `localhost` resolves to 127.0.0.1 (TcpListener does not accept hostnames).
fn bind_addr(host: &str) -> std::net::IpAddr {
    match host {
        "localhost" => std::net::IpAddr::from([127, 0, 0, 1]),
        other => other
            .parse()
            .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1])),
    }
}

/// Constant-time compare: return at once on length mismatch (length is not secret); content
/// accumulates XOR per byte — bit-level short-circuit would leak "which byte was wrong".
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |diff, (x, y)| diff | (x ^ y))
        == 0
}

/// Route table. Pulled out so tests can hit it directly without starting a listener.
pub fn router(gate: Gate) -> Router {
    Router::new()
        .route("/api/data", get(api::data))
        .route("/api/capability", get(api::connections::capability_list))
        .route(
            "/api/connector/settings",
            get(api::connections::connector_settings),
        )
        .route(
            "/api/connector/settings/save",
            post(api::connections::connector_settings_save),
        )
        .route(
            "/api/connector/oauth/start",
            post(api::connections::connector_oauth_start),
        )
        .route(
            "/api/capability/probe",
            post(api::connections::capability_probe),
        )
        .route("/api/config", get(api::config_get).post(api::config_set))
        .route("/api/memory/remember", post(api::remember))
        .route("/api/inbox", get(api::inbox))
        .route("/api/vault", get(api::vault::list).post(api::vault::put))
        .route("/api/vault/remove", post(api::vault::remove))
        .route(
            "/api/connections/lark",
            get(api::connections::lark_get).post(api::connections::lark_save),
        )
        .route(
            "/api/connections/lark/claim",
            post(api::connections::lark_claim),
        )
        .route("/api/model/catalog", get(api::model_catalog))
        .route(
            "/api/session/permission",
            get(api::permission_get).post(api::permission_set),
        )
        .route("/api/events", get(api::events))
        .route("/api/notifications", get(api::notifications))
        .route("/api/gateway/rpc", post(api::gateway::rpc))
        .route("/api/session/main", get(api::main_session))
        .route("/api/interaction", post(api::interaction))
        .route("/api/session", get(api::session))
        .route("/api/session/meta", get(api::session_meta))
        .route("/api/session/update", post(api::session_update))
        .route("/api/turns", get(api::turns))
        .route("/api/turn/steer", post(api::steer))
        .route("/api/turn/follow-up", post(api::follow_up))
        .route("/api/approval", post(api::approval))
        .route("/api/approval/pending", get(api::pending_approvals))
        .route("/api/interrupt", post(api::interrupt))
        .route("/api/browser", get(api::browser::list))
        .route("/api/browser/start", post(api::browser::start))
        .route(
            "/api/approval/notice",
            get(api::devices::notice).post(api::devices::decision),
        )
        .route("/api/approval/push-key", post(api::devices::push_key))
        .route(
            "/api/approval/push-subscribe",
            post(api::devices::subscribe),
        )
        .route("/api/device", get(api::devices::list))
        .route("/api/device/pair", post(api::devices::pair))
        .route("/api/device/revoke", post(api::devices::revoke))
        .route("/api/desktop/status", get(api::desktop::status))
        .route("/api/desktop/select", post(api::desktop::select))
        .route("/api/desktop/observe", post(api::desktop::observe))
        .route("/api/desktop/act", post(api::desktop::act))
        .route("/api/desktop/takeover", post(api::desktop::takeover))
        .route("/api/desktop/model", post(api::desktop::model))
        .route("/api/desktop/frame", post(api::desktop::frame))
        .route("/api/desktop/share", post(api::desktop::share))
        .route("/api/executor/control", get(api::browser::executor_status))
        .route("/api/executor/stop", post(api::browser::stop))
        .route("/api/executor/release", post(api::browser::release))
        .route("/api/browser/input", post(api::browser::input))
        .route("/api/browser/downloads", get(api::browser::downloads))
        .route("/api/browser/download", post(api::browser::download))
        .route("/api/browser/control", post(api::browser::control))
        .route("/api/browser/frame", get(api::browser::frame))
        .route("/api/tasks", get(api::tasks))
        .route("/api/task", post(api::task))
        .route("/api/task/events", get(api::task_events))
        .route("/api/task/integrate", post(api::task_integrate))
        .route("/api/task/git-checkpoint", post(api::task_git_checkpoint))
        .route(
            "/api/task/git-checkpoint-policy",
            get(api::task_git_checkpoint_policy).post(api::task_git_checkpoint_policy_set),
        )
        .route("/api/turn-operation", post(api::turn_operation))
        .route("/api/usage", get(api::usage))
        .route("/api/approvals", get(api::approvals))
        .route(
            "/api/responsibilities/reports",
            get(api::responsibilities::reports),
        )
        .route("/api/schedules/jobs", get(api::schedule_jobs))
        .route(
            "/api/responsibilities",
            get(api::responsibilities::list).post(api::responsibilities::register),
        )
        .route(
            "/api/responsibilities/prepare",
            post(api::responsibilities::prepare),
        )
        .route(
            "/api/responsibilities/pause",
            post(api::responsibilities::set_paused),
        )
        .route("/api/schedules/save", post(api::schedule_save))
        .route("/api/schedules/remove", post(api::schedule_remove))
        .route("/api/upload", post(api::upload))
        .route("/api/fs", get(api::fs_browse))
        .route("/api/fs/mkdir", post(api::fs_mkdir))
        .route(
            "/api/fs/recents",
            get(api::fs_recents).post(api::fs_recents_record),
        )
        .route("/api/model/profiles", get(api::model_profiles))
        .route("/api/model/save", post(api::model_save))
        .route("/api/model/remove", post(api::model_remove))
        .route("/api/model/routes", get(api::model_routes))
        .route("/api/model/routes/save", post(api::model_routes_save))
        .route("/api/skills", get(api::skills))
        .route("/api/skills/install", post(api::skills_install))
        .route("/api/skills/remove", post(api::skills_remove))
        .route("/api/mcp", get(api::mcp))
        .route("/api/connector/tools", get(api::connections::tools))
        .route("/api/connector/tools/set", post(api::connections::set_tool))
        .route("/api/connector/catalog", get(api::connections::catalog))
        .route("/api/mcp/save", post(api::mcp_save))
        .route("/api/mcp/remove", post(api::mcp_remove))
        .route("/api/ledger/turns", get(memory_api::ledger_turns))
        .route("/api/ledger/turn", get(memory_api::ledger_turn))
        .route("/api/memory/index", get(memory_api::memory_index))
        .route("/api/memory/page", get(memory_api::memory_page))
        .route("/api/context", get(memory_api::context))
        .route("/api/skills/stats", get(memory_api::skill_stats))
        .route("/api/compact", post(memory_api::compact))
        .route("/api/run", post(api::run))
        .route_layer(axum::middleware::from_fn_with_state(
            gate.clone(),
            authorize,
        ))
        .route("/api/hook/{name}", post(api::hook))
        .route(
            "/api/device/report",
            post(api::devices::report).layer(axum::extract::DefaultBodyLimit::max(2048)),
        )
        .route("/healthz", get(api::healthz))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(axum::extract::DefaultBodyLimit::max(25 * 1024 * 1024))
        .with_state(gate)
}

/// Wrap "an error in the response" as "a notification in the stream".
///
/// The two types have identical fields; separate only because the JSON-RPC envelopes differ
/// (one is `error`, the other is `turn/notification`'s params). After the swap the browser
/// only recognizes one shape: turn failed mid-run or never started — either way it receives
/// a notification with `type: "error"`.
pub(crate) fn as_notification(error: ErrorObject) -> ServerNotification {
    ServerNotification::Error(ErrorNotification {
        code: error.code,
        message: error.message,
        data: error.data,
    })
}

fn port() -> u16 {
    std::env::var("JEEVES_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}
