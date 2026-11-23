//! Notifier (M3a): pushes "things a human should see" out of the process.
//!
//! Event kinds: **approval requests** (the human is off-screen — they can't click Slack
//! but should know to come back), **job completion** (a 7x24 background command finished
//! or died), **goal endings** (satisfied/impossible/round-limit/blocked — the loop
//! stopped, a human should know). Configured completion notices are persisted before
//! network delivery and retried across restarts. Approval prompts remain ephemeral.
//! Network failures never block a turn; persistence failures are reported to stderr.
//!
//! Egress target: the URL comes from `<home>/notify.json` (admin-configured). **Deliberately
//! no** `fetch_url`-style SSRF guardrails — those guard against "the model points, we
//! fetch"; here the URL is hand-configured by the admin, and pointing at an intranet
//! self-hosted endpoint (ntfy/Gotify) is legitimate use, not an attack surface.
//! Switch: `JEEVES_NOTIFY=1` (off by default) — external sends are a user-pressed decision,
//! same philosophy as `JEEVES_HOOKS`/`JEEVES_DELEGATE`.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

/// Things a human should see.
#[derive(Debug, Clone)]
pub enum NotifyEvent {
    ApprovalRequested {
        turn_id: String,
        tool: String,
        args_preview: String,
        reason: String,
    },
    JobFinished {
        id: String,
        command: String,
        status: String,
        exit_code: Option<i32>,
    },
    ScheduleRan {
        name: String,
        status: String,
        preview: String,
    },
    TaskFinished {
        id: String,
        title: String,
        status: String,
        reason: String,
    },
    GoalFinished {
        session_id: String,
        status: String,
        reason: String,
    },
}

impl NotifyEvent {
    /// One human-readable sentence — the body of the Slack message.
    pub fn message(&self) -> String {
        match self {
            NotifyEvent::ApprovalRequested {
                tool,
                args_preview,
                reason,
                ..
            } => format!(
                "🔔 Jeeves asks for approval: `{tool}` {args_preview}\n{reason}\n(approve in the cockpit or the terminal)"
            ),
            NotifyEvent::JobFinished {
                id,
                command,
                status,
                exit_code,
            } => match exit_code {
                Some(code) => {
                    format!(
                        "⚙️ Jeeves background job {id} finished ({status}, exit {code}): `{command}`"
                    )
                }
                None => format!("⚙️ Jeeves background job {id} finished ({status}): `{command}`"),
            },
            NotifyEvent::ScheduleRan {
                name,
                status,
                preview,
            } => format!("⏰ Jeeves scheduled job '{name}' {status}: {preview}"),
            NotifyEvent::TaskFinished {
                title,
                status,
                reason,
                ..
            } => format!("Jeeves task '{title}' stopped ({status}): {reason}"),
            NotifyEvent::GoalFinished { status, reason, .. } => {
                format!("🎯 Jeeves's goal stopped ({status}): {reason}")
            }
        }
    }
}

/// A bounded, non-queued delivery sees the receiver's response directly.
pub type SingleDelivery<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

/// Notifier. Ordinary completion notices persist before background delivery.
pub trait Notifier: Send + Sync {
    fn notify(&self, event: NotifyEvent);
    /// Accepted for delivery (configured completion notices are persisted before success).
    fn enqueue(&self, event: NotifyEvent) -> Result<(), String> {
        self.notify(event);
        Ok(())
    }
    /// Stable producer receipt. Persistent adapters deduplicate it across worker restarts.
    fn enqueue_once(&self, _key: &str, event: NotifyEvent) -> Result<(), String> {
        self.enqueue(event)
    }
    /// Attempt an explicitly selected scheduled proposal once, without the durable retry queue.
    /// Returning success means the receiver accepted it, not merely that it entered a queue.
    fn send_once(&self, _text: String) -> SingleDelivery<'_> {
        Box::pin(async { Err("notifier does not support one-shot delivery".into()) })
    }
    /// Opaque destination pointer; never contains the webhook credential.
    fn delivery_target(&self) -> Option<String> {
        None
    }
}

/// Slack incoming webhook (also compatible with self-hosted endpoints taking `{"text": …}` JSON).
pub struct Slack {
    url: String,
    client: reqwest::Client,
    queue: Option<Arc<crate::notify_queue::Queue>>,
}

impl Slack {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::new(),
            queue: None,
        }
    }
}

impl Notifier for Slack {
    fn send_once(&self, text: String) -> SingleDelivery<'_> {
        Box::pin(async move {
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .map_err(|_| "notification transport unavailable".to_string())?;
            let response = client
                .post(&self.url)
                .json(&serde_json::json!({ "text": text }))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|_| "notification transport failed; receipt unknown".to_string())?;
            if !response.status().is_success() {
                return Err(format!(
                    "notification receiver returned HTTP {}",
                    response.status()
                ));
            }
            Ok(())
        })
    }
    fn notify(&self, event: NotifyEvent) {
        if let Err(e) = self.enqueue(event) {
            eprintln!("(jeeves) notification could not be persisted: {e}");
        }
    }
    fn delivery_target(&self) -> Option<String> {
        self.queue.as_ref().map(|queue| queue.target())
    }
    fn enqueue_once(&self, key: &str, event: NotifyEvent) -> Result<(), String> {
        match &self.queue {
            Some(queue) => queue
                .enqueue_once(key, &event.message())
                .map_err(|e| e.to_string()),
            None => Err("Durable notification queue unavailable".into()),
        }
    }
    fn enqueue(&self, event: NotifyEvent) -> Result<(), String> {
        let text = event.message();
        if !matches!(event, NotifyEvent::ApprovalRequested { .. })
            && let Some(queue) = &self.queue
        {
            return queue.enqueue(&text).map_err(|e| e.to_string());
        }
        let url = self.url.clone();
        let client = self.client.clone();
        tokio::spawn(async move {
            let result = client
                .post(&url)
                .json(&serde_json::json!({ "text": text }))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await;
            match result {
                Ok(resp) if !resp.status().is_success() => {
                    eprintln!(
                        "(jeeves) the notification did not get through: HTTP {}",
                        resp.status()
                    );
                }
                Err(_) => eprintln!("(jeeves) notification transport failed"),
                Ok(_) => {}
            }
        });
        Ok(())
    }
}

/// `<home>/notify.json`: `{ "slack": "https://hooks.slack.com/services/…" }`.
/// Same convention as `mcp.json`/`hooks.json`/`schedules.json`: one file per home,
/// naturally per-user in hub mode. File missing, `JEEVES_NOTIFY` off, field empty — all
/// mean no notifier (return None, silently: "not configured" is the norm, not an error).
pub fn load(home: &Path, enabled: bool) -> Option<Arc<dyn Notifier>> {
    if !enabled {
        return None;
    }
    let path = home.join("notify.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| {
            eprintln!("(jeeves) notify.json could not be read; notifications stay off: {e}")
        })
        .ok()?;
    let url = value.get("slack")?.as_str()?.trim().to_string();
    if url.is_empty() {
        return None;
    }
    eprintln!(
        "(jeeves) notifications enabled: Slack webhook (from {})",
        path.display()
    );
    let queue = crate::notify_queue::Queue::new(home, &url);
    queue.start();
    let mut slack = Slack::new(url);
    slack.queue = Some(queue);
    Some(Arc::new(slack))
}
