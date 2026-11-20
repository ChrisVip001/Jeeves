//! Both event sources re-enter the same task recovery and accounting driver.
use crate::Server;
use jeeves_state::{
    external_waits::{ExternalWaits, InputWait},
    waits::{JobWait, JobWaits},
};

pub(crate) enum Wake {
    Jobs(JobWait),
    Input(InputWait),
}
impl Wake {
    pub(crate) fn checkpoint_status(&self) -> &'static str {
        match self {
            Self::Jobs(_) => "waiting_jobs",
            Self::Input(_) => "waiting_input",
        }
    }
    pub(crate) fn task_id(&self) -> &str {
        match self {
            Self::Jobs(wait) => &wait.task_id,
            Self::Input(wait) => &wait.task_id,
        }
    }
    pub(crate) async fn claim(&self, server: &Server, messages: &str) -> anyhow::Result<bool> {
        match self {
            Self::Jobs(wait) => {
                JobWaits::new(server.pool.clone())
                    .claim(wait, messages)
                    .await
            }
            Self::Input(wait) => {
                ExternalWaits::new(server.pool.clone())
                    .claim(wait, messages)
                    .await
            }
        }
    }
}

/// Record a wake under its source key. `true` means deliver it now. A repeat of a key that is
/// no longer pending is reported on stderr and as a notification, never dropped silently.
pub(crate) async fn register(server: &Server, key: &str, target: &str, payload: &str) -> bool {
    use jeeves_state::{WakeInbox, WakeOutcome};
    let inbox = WakeInbox::new(server.pool.clone());
    match inbox.wake(key, target, payload).await {
        Ok(WakeOutcome::Queued) => true,
        Ok(WakeOutcome::Duplicate) => {
            let pending = inbox
                .pending()
                .await
                .map(|rows| rows.iter().any(|row| row.source_key == key))
                .unwrap_or(false);
            if !pending {
                eprintln!("(jeeves) duplicate wake skipped: {key}");
                if let Some(notifier) = server.notifier() {
                    notifier.notify(jeeves_tools::notify::NotifyEvent::ScheduleRan {
                        name: target.to_string(),
                        status: "duplicate wake skipped".into(),
                        preview: key.to_string(),
                    });
                }
            }
            pending
        }
        Err(error) => {
            eprintln!("(jeeves) wake inbox unavailable for {key}: {error}");
            true
        }
    }
}

pub(crate) async fn settle(server: &Server, key: &str, reconcile: bool) {
    let inbox = jeeves_state::WakeInbox::new(server.pool.clone());
    let result = if reconcile {
        inbox.reconcile(key).await
    } else {
        inbox.delivered(key).await
    };
    if let Err(error) = result {
        eprintln!("(jeeves) wake inbox could not settle {key}: {error}");
    }
}

/// Startup: a scheduled firing still pending was claimed but never settled. It is never
/// redelivered; it waits for a human (`schedule acknowledge`) without blocking other jobs.
pub(crate) async fn reconcile_unsettled_schedules(server: &Server) {
    let inbox = jeeves_state::WakeInbox::new(server.pool.clone());
    if let Ok(rows) = inbox.pending().await {
        for row in rows
            .iter()
            .filter(|row| row.source_key.starts_with("schedule:"))
        {
            settle(server, &row.source_key, true).await;
        }
    }
}
