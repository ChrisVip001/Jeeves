use super::*;
use chrono::Duration;
use jeeves_state::WakeInbox;
use jeeves_tools::notify::{Notifier, NotifyEvent};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tempfile::TempDir;

#[derive(Default)]
struct Notices(Mutex<Vec<String>>);

impl Notifier for Notices {
    fn notify(&self, event: NotifyEvent) {
        if let NotifyEvent::ScheduleRan { name, status, .. } = event {
            self.0.lock().unwrap().push(format!("{name}/{status}"));
        }
    }
}

async fn boot(home: &TempDir, notices: Arc<Notices>) -> Server {
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    let mut server = Server::boot(
        pool,
        jeeves_config::Settings {
            home: home.path().to_owned(),
            ..Default::default()
        },
    )
    .await;
    server.notifier = Some(notices);
    server
}

fn start() -> DateTime<Utc> {
    "2030-02-05T12:00:00Z".parse().unwrap()
}

fn contract() -> ResponsibilityContract {
    serde_json::from_value(json!({
        "id":"weekly", "purpose":"Prepare the fixture report", "sources":["fixture:notes"],
        "cron":"* * * * *", "expiresAt":"2030-02-05T12:05:00Z",
        "budget":{"tokens":200,"tools":2}
    }))
    .unwrap()
}

fn pause(paused: bool) -> ResponsibilitySetPausedParams {
    ResponsibilitySetPausedParams {
        id: "weekly".into(),
        paused,
    }
}

#[tokio::test]
async fn invalid_contract_is_rejected_before_any_write() {
    let home = tempfile::tempdir().unwrap();
    let server = boot(&home, Default::default()).await;
    for (field, bad) in [
        ("budget", json!({"tokens":100,"tools":0})),
        ("budget", json!({"wallMs":-1})),
        ("budget", json!({})),
        ("cron", json!("60 * * * *")),
        ("expiresAt", json!("2030-02-05T12:00:00Z")),
        ("expiresAt", json!("next Friday")),
        ("purpose", json!(" ")),
        ("id", json!("bad/id")),
        ("sources", json!(["notes", " "])),
    ] {
        let mut wire = serde_json::to_value(contract()).unwrap();
        wire[field] = bad;
        let request = serde_json::from_value(wire).unwrap();
        assert_eq!(
            register(&server, request, start()).await.unwrap_err().code,
            codes::INVALID_PARAMS,
            "{field}"
        );
    }
    assert!(list(&server, start())
        .await
        .unwrap()
        .responsibilities
        .is_empty());
    register(&server, contract(), start()).await.unwrap();
    let mut other = contract();
    other.purpose = "replacement".into();
    assert_eq!(
        register(&server, other, start()).await.unwrap_err().code,
        codes::INVALID_PARAMS
    );
    let stored = list(&server, start()).await.unwrap().responsibilities;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].contract.purpose, contract().purpose);
}

#[tokio::test]
async fn reviewed_weekly_contract_needs_a_separate_unexpired_registration() {
    let home = tempfile::tempdir().unwrap();
    let server = boot(&home, Default::default()).await;
    let now = start();
    let mut request = contract();
    request.sources = vec!["  https://example.org/report  ".into()];
    request.weekly_report = Some(jeeves_protocol::FeishuWeeklyReport {
        task_id: "  research ".into(),
        recipient: " group ".into(),
    });
    let reviewed = prepare(request, now).unwrap();
    let report = reviewed.weekly_report.as_ref().unwrap();
    assert_eq!(report.recipient, "group");
    assert_eq!(report.task_id, "research");
    assert_eq!(reviewed.sources, ["https://example.org/report"]);
    let stored = register(&server, reviewed.clone(), now + Duration::minutes(5)).await;
    assert_eq!(stored.unwrap_err().code, codes::INVALID_PARAMS);
    assert!(Responsibilities(&server.pool)
        .list()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn concurrent_claim_restart_pause_and_expiry_respect_the_human_contract() {
    let home = tempfile::tempdir().unwrap();
    let notices = Arc::new(Notices::default());
    let now = start();
    {
        let server = boot(&home, notices.clone()).await;
        register(&server, contract(), now).await.unwrap();
        let (a, b) = tokio::join!(tick(&server, now), tick(&server, now));
        a.unwrap();
        b.unwrap();
        let pending = WakeInbox::new(server.pool.clone()).pending().await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload, contract().purpose);
        assert_eq!(pending[0].target_task, "responsibility:weekly");
        server.pool.close().await;
    }
    let server = boot(&home, notices.clone()).await;
    let inbox = WakeInbox::new(server.pool.clone());
    tick(&server, now + Duration::seconds(20)).await.unwrap();
    assert_eq!(inbox.pending().await.unwrap().len(), 1);
    for _ in 0..2 {
        set_paused(&server, pause(true), now).await.unwrap();
    }
    server.pool.close().await;
    let server = boot(&home, notices.clone()).await;
    let inbox = WakeInbox::new(server.pool.clone());
    let minute = |n| now + Duration::minutes(n);
    tick(&server, minute(1)).await.unwrap();
    assert!(inbox.pending().await.unwrap().is_empty());
    assert_eq!(inbox.needs_reconcile().await.unwrap().len(), 1);
    set_paused(&server, pause(false), minute(2)).await.unwrap();
    tick(&server, minute(1)).await.unwrap();
    assert!(inbox.pending().await.unwrap().is_empty());
    tick(&server, minute(3)).await.unwrap();
    tick(&server, minute(2)).await.unwrap();
    assert_eq!(inbox.pending().await.unwrap().len(), 1);
    tick(&server, minute(5)).await.unwrap();
    tick(&server, minute(6)).await.unwrap();
    assert!(inbox.pending().await.unwrap().is_empty());
    assert_eq!(inbox.needs_reconcile().await.unwrap().len(), 2);
    assert!(set_paused(&server, pause(false), minute(6)).await.is_err());
    let stored = list(&server, minute(6)).await.unwrap().responsibilities;
    assert_eq!(stored[0].status, ResponsibilityStatus::Expired);
    assert_eq!(stored[0].last_due_at, Some(minute(3).to_rfc3339()));
    assert_eq!(
        *notices.0.lock().unwrap(),
        [
            "responsibility:weekly/paused",
            "responsibility:weekly/expired"
        ]
    );
}

#[tokio::test]
async fn first_tick_after_downtime_expires_old_work_and_only_claims_matching_duties() {
    use chrono::Timelike;
    let home = tempfile::tempdir().unwrap();
    let notices = Arc::new(Notices::default());
    let now = start();
    let restart = now + Duration::minutes(7);
    {
        let server = boot(&home, notices.clone()).await;
        register(&server, contract(), now).await.unwrap();
        tick(&server, now).await.unwrap();
        let mut continuing = contract();
        continuing.id = "other".into();
        continuing.expires_at = (restart + Duration::hours(1)).to_rfc3339();
        let next = (restart + Duration::minutes(1)).with_timezone(&Local);
        continuing.cron = format!("{} * * * *", next.minute());
        register(&server, continuing, now).await.unwrap();
        server.pool.close().await;
    }
    let server = boot(&home, notices.clone()).await;
    tick(&server, restart).await.unwrap();
    let inbox = WakeInbox::new(server.pool.clone());
    assert!(inbox.pending().await.unwrap().is_empty());
    let held = inbox.needs_reconcile().await.unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].target_task, "responsibility:weekly");
    tick(&server, restart + Duration::minutes(1)).await.unwrap();
    let pending = inbox.pending().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].target_task, "responsibility:other");
    assert_eq!(
        *notices.0.lock().unwrap(),
        ["responsibility:weekly/expired"]
    );
}

#[tokio::test]
async fn failed_wake_insert_rolls_back_the_claim_for_retry() {
    let home = tempfile::tempdir().unwrap();
    let server = boot(&home, Default::default()).await;
    register(&server, contract(), start()).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_wake BEFORE INSERT ON wake_inbox BEGIN SELECT RAISE(ABORT, 'fixture failure'); END").execute(&server.pool).await.unwrap();
    assert!(tick(&server, start()).await.is_err());
    assert_eq!(
        list(&server, start()).await.unwrap().responsibilities[0].last_due_at,
        None
    );
    sqlx::query("DROP TRIGGER fail_wake")
        .execute(&server.pool)
        .await
        .unwrap();
    tick(&server, start()).await.unwrap();
    assert_eq!(
        WakeInbox::new(server.pool.clone())
            .pending()
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn editing_fences_the_old_clock_and_preserves_each_claimed_revision() {
    let home = tempfile::tempdir().unwrap();
    let server = boot(&home, Default::default()).await;
    let now = start();
    let first = register(&server, contract(), now).await.unwrap();
    tick(&server, now).await.unwrap();
    let mut changed = first.contract;
    changed.purpose = "New purpose".into();
    let edit = jeeves_protocol::ResponsibilityAmendParams {
        expected_revision: first.revision,
        contract: changed,
    };
    let (a, b) = tokio::join!(amend(&server, edit.clone(), now), amend(&server, edit, now));
    assert_ne!(a.is_ok(), b.is_ok());
    let later = now + Duration::minutes(1);
    assert!(!Responsibilities(&server.pool)
        .claim("weekly", 1, later.timestamp_millis())
        .await
        .unwrap());
    tick(&server, later).await.unwrap();
    let rows:Vec<(String,i32,String)>=sqlx::query_as("SELECT w.state,h.revision,json_extract(h.definition,'$.purpose') FROM wake_inbox w JOIN responsibility_revisions h ON w.target_task='responsibility:' || h.responsibility_id AND w.responsibility_revision=h.revision ORDER BY w.rowid")
        .fetch_all(&server.pool).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ("reconcile".into(), 1, contract().purpose),
            ("pending".into(), 2, "New purpose".into())
        ]
    );
}
