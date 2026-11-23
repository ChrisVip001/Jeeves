//! Read-only polling for explicitly opted-in task input conditions. It can only release
//! an existing durable wait, never create a task, send mail or widen permissions.
use crate::Server;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use jeeves_state::external_waits::{ExternalWaits, InputCondition};
use serde_json::{Value, json};

pub(crate) enum ConnectorWait {
    Email,
    Calendar,
}

pub(crate) fn kind(source: &str, key: &str) -> Option<ConnectorWait> {
    match (source, key) {
        ("connector:email", "new-unread") => Some(ConnectorWait::Email),
        ("connector:calendar", "upcoming-15m") => Some(ConnectorWait::Calendar),
        _ => None,
    }
}

fn calendar_candidate(
    events: &[jeeves_connector::calendar::CalendarEvent],
    now: DateTime<Utc>,
) -> Option<Value> {
    let end = now + Duration::minutes(15);
    events
        .iter()
        .filter(|e| !e.all_day)
        .filter_map(|e| {
            let start = DateTime::parse_from_rfc3339(&e.start)
                .expect("calendar event start is RFC3339")
                .with_timezone(&Utc);
            (start >= now && start <= end).then_some((start, &e.uid))
        })
        .min_by_key(|(start, _)| *start)
        .map(|(start, uid)| json!({"uid":uid,"start":start.to_rfc3339()}))
}

pub(crate) async fn poll(
    server: &Server,
    rows: &ExternalWaits,
    condition: &InputCondition,
    kind: ConnectorWait,
) -> Result<bool> {
    static POLLS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);
    let Ok(_permit) = POLLS.try_acquire() else {
        return Ok(false);
    };
    let home = server.settings().home;
    let config = jeeves_connector::config::load(&home)?;
    let data = match kind {
        ConnectorWait::Email => {
            let config = config.email.context("email is not configured")?;
            let digest = jeeves_state::ledger::digest_of(
                format!(
                    "{}:{}:{:?}:{}:{}",
                    config.imap.host,
                    config.imap.port,
                    config.imap.security,
                    config.username,
                    config.password
                )
                .as_bytes(),
            );
            let baseline = rows.mail_cursor(&condition.condition_id).await?;
            let cursor = if let Some((account, generation, uid)) = baseline {
                anyhow::ensure!(account == digest, "mail account changed; review this wait");
                Some((u32::try_from(generation)?, u32::try_from(uid)?))
            } else {
                None
            };
            let observed = jeeves_connector::email::poll_inbox(&home, &config, cursor).await?;
            if cursor.is_none() {
                rows.establish_mail_cursor(
                    &condition.condition_id,
                    &digest,
                    observed.uid_validity,
                    observed.next_uid,
                )
                .await?;
                eprintln!(
                    "(jeeves) task {}: new-mail baseline established",
                    condition.task_id
                );
            }
            observed
                .unread_uid
                .map(|uid| json!({"mailbox":"INBOX","uidValidity":observed.uid_validity,"uid":uid}))
        }
        ConnectorWait::Calendar => {
            let config = config.calendar.context("calendar is not configured")?;
            let digest = jeeves_state::ledger::digest_of(
                format!(
                    "{}:{:?}:{:?}:{}",
                    config.url, config.username, config.password, config.timezone
                )
                .as_bytes(),
            );
            if let Some((account, _, _)) = rows.mail_cursor(&condition.condition_id).await? {
                anyhow::ensure!(
                    account == digest,
                    "calendar account changed; review this wait"
                );
            } else {
                rows.establish_mail_cursor(&condition.condition_id, &digest, 0, 0)
                    .await?;
            }
            let now = Utc::now();
            let events = jeeves_connector::calendar::read_events(
                &home,
                &config,
                now,
                now + Duration::minutes(15),
                &config.timezone,
            )
            .await?;
            calendar_candidate(&events, now)
        }
    };
    let Some(data) = data else {
        return Ok(false);
    };
    let event =
        jeeves_state::ledger::digest_of(format!("{}:{}", condition.condition_id, data).as_bytes());
    let lock = server.session_write_lock(&format!("task-control:{}", condition.task_id));
    let _guard = lock.lock().await;
    let task = server
        .tasks
        .get(&condition.task_id)
        .await?
        .context("task is unavailable")?;
    if task.status != jeeves_state::TaskStatus::Suspended
        || task.suspend_reason.as_deref() != Some(jeeves_state::external_waits::INPUT_WAIT_REASON)
    {
        return Ok(false);
    }
    let fresh = rows
        .signal(
            &condition.condition_id,
            &condition.source,
            &condition.event_key,
            &event,
            &data.to_string(),
        )
        .await?;
    if fresh {
        eprintln!(
            "(jeeves) task {}: authorized connector input arrived",
            condition.task_id
        );
    }
    Ok(fresh)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calendar_wait_matches_only_upcoming_timed_occurrences() {
        let now = DateTime::parse_from_rfc3339("2026-11-19T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let events = jeeves_connector::calendar::events_in_range(
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:past\r\nDTSTART:20261119T075900Z\r\nDTEND:20261119T080200Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:all-day\r\nDTSTART;VALUE=DATE:20261119\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:later\r\nDTSTART:20261119T081600Z\r\nDTEND:20261119T082000Z\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:wanted\r\nDTSTART:20261119T080500Z\r\nDTEND:20261119T081000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            now - Duration::hours(1),
            now + Duration::hours(1),
            "UTC",
        )
        .unwrap();
        assert_eq!(calendar_candidate(&events, now).unwrap()["uid"], "wanted");
        assert!(kind("connector:email", "all").is_none());
    }

    #[tokio::test]
    async fn mail_baseline_survives_reopen_and_cannot_be_silently_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let pool = jeeves_state::open(&path).await.unwrap();
        let rows = ExternalWaits::new(pool.clone());
        rows.establish_mail_cursor("condition", "account-a", 41, 9)
            .await
            .unwrap();
        drop(rows);
        pool.close().await;
        let rows = ExternalWaits::new(jeeves_state::open(&path).await.unwrap());
        rows.establish_mail_cursor("condition", "account-b", 42, 10)
            .await
            .unwrap();
        assert_eq!(
            rows.mail_cursor("condition").await.unwrap(),
            Some(("account-a".into(), 41, 9))
        );
    }

    #[tokio::test]
    async fn a_calendar_fixture_wakes_only_the_explicitly_waiting_task() {
        use crate::input_wake_tests::{fixture, rpc};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (home, server, _model) = fixture().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/calendar.ics", listener.local_addr().unwrap());
        let start = (Utc::now() + Duration::minutes(5)).format("%Y%m%dT%H%M%SZ");
        let end = (Utc::now() + Duration::minutes(10)).format("%Y%m%dT%H%M%SZ");
        let past = (Utc::now() - Duration::minutes(5)).format("%Y%m%dT%H%M%SZ");
        let mut ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Fixture//EN\r\n".to_string();
        for index in 0..201 {
            ics.push_str(&format!("BEGIN:VEVENT\r\nUID:past-{index}\r\nDTSTART:{past}\r\nDTEND:{end}\r\nEND:VEVENT\r\n"));
        }
        ics.push_str(&format!("BEGIN:VEVENT\r\nUID:fixture-calendar\r\nDTSTAMP:20260101T000000Z\r\nDTSTART:{start}\r\nDTEND:{end}\r\nSUMMARY:Fixture\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"));
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/calendar\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ics}",ics.len()).as_bytes()).await.unwrap();
        });
        std::fs::write(
            home.path().join("connectors.json"),
            json!({"calendar":{"url":url,"timezone":"UTC"}}).to_string(),
        )
        .unwrap();
        server
            .tasks
            .create(jeeves_state::NewTask {
                id: "cal-task",
                session_id: Some("cal-chat"),
                title: "calendar fixture",
                goal_condition: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 3,
                tier: None,
            })
            .await
            .unwrap();
        rpc(&server,"task/wait",json!({"taskId":"cal-task","source":"connector:calendar","eventKey":"upcoming-15m","description":"Wait for a timed occurrence"})).await.unwrap();
        rpc(
            &server,
            "task/start",
            json!({"taskId":"cal-task","message":"Wait for upcoming event"}),
        )
        .await
        .unwrap();
        let rows = ExternalWaits::new(server.pool.clone());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if rows
                    .get("cal-task")
                    .await
                    .unwrap()
                    .unwrap()
                    .consumed_run_id
                    .is_some()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        let data = rows
            .get("cal-task")
            .await
            .unwrap()
            .unwrap()
            .payload_json
            .unwrap();
        assert!(data.contains("fixture-calendar"));
        assert!(
            rows.get("input-task")
                .await
                .unwrap()
                .unwrap()
                .event_id
                .is_none()
        );
    }
}
