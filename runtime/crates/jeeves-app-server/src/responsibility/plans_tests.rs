use super::*;
use jeeves_protocol::{ResponsibilityDecision, ResponsibilityEvent};
use serde_json::json;

async fn test_server(home: &tempfile::TempDir) -> Server {
    crate::open(&jeeves_config::Settings {
        home: home.path().into(),
        ..Default::default()
    })
    .await
    .unwrap()
}

fn occurrence(now: DateTime<Utc>) -> ResponsibilityEvent {
    serde_json::from_value(json!({
        "source":"inbox","eventType":"received","eventId":"m1","cursor":"one",
        "occurredAt":now.to_rfc3339(),"fields":{"key":"sample"}
    }))
    .unwrap()
}

#[tokio::test]
async fn a_plan_stays_in_review_until_a_human_activates_it() {
    let home = tempfile::tempdir().unwrap();
    let server = test_server(&home).await;
    let tool = reviews::tool(
        server.clone(),
        "review-one".into(),
        "Remind me in 30 minutes".into(),
    );
    let ctx = server.tool_ctx(crate::MAIN_SESSION, None, None);
    let fields = json!({
        "action":"create","id":"remind","fields":{"purpose":"A reminder","plan":{
            "trigger":{"kind":"after","delaySeconds":1800},
            "action":{"kind":"reminder","version":1,"config":{"text":"Take a break"}},
            "delivery":{"kind":"chat"}}}
    });
    (tool.handler)(ctx, fields).await.unwrap();
    assert!(list(&server, Utc::now())
        .await
        .unwrap()
        .responsibilities
        .is_empty());
    let before = Utc::now().timestamp_millis();
    reviews::decide(
        &server,
        ResponsibilityDecision {
            id: "review-one".into(),
            accept: true,
        },
    )
    .await
    .unwrap();
    let due: i64 = sqlx::query_scalar("SELECT next_due_ms FROM responsibilities WHERE id='remind'")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert!((before + 1_800_000..=Utc::now().timestamp_millis() + 1_800_000).contains(&due));
    let at = DateTime::from_timestamp_millis(due).unwrap();
    let (a, b) = tokio::join!(tick(&server, at), tick(&server, at));
    a.unwrap();
    b.unwrap();
    let (work, duplicate) = tokio::join!(
        jeeves_state::responsibility_runs::take(&server.pool, due),
        jeeves_state::responsibility_runs::take(&server.pool, due),
    );
    let work = match (work.unwrap(), duplicate.unwrap()) {
        (Some(work), None) | (None, Some(work)) => work,
        _ => panic!("Expected one worker to claim the slot"),
    };
    let (sink, _rx) = crate::EventSink::channel();
    worker::process(&server, &sink, &work).await.unwrap();
    let output: Vec<String> = sqlx::query_scalar("SELECT content FROM chat_log WHERE source=?")
        .bind(work.wake_key)
        .fetch_all(&server.pool)
        .await
        .unwrap();
    assert_eq!(output, ["Take a break"]);
}

#[tokio::test]
async fn event_admission_preserves_the_cursor_when_a_wake_cannot_commit() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("webhooks.json"),
        br#"{"hooks":[{"name":"inbox","signature":true,"event_type":"received"}]}"#,
    )
    .unwrap();
    let server = test_server(&home).await;
    let now = Utc::now();
    register(&server,serde_json::from_value(json!({
        "id":"event-duty","purpose":"Reaction", "expiresAt":(now+chrono::Duration::days(1)).to_rfc3339(),
        "budget":{"tools":1},"plan":{
            "trigger":{"kind":"event","source":"inbox","eventType":"received"},
            "action":{"kind":"reminder","version":1,"config":{"text":"Received"}},
            "delivery":{"kind":"chat"}}
    })).unwrap(),now-chrono::Duration::milliseconds(100)).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_event BEFORE INSERT ON wake_inbox BEGIN SELECT raise(abort,'fixture'); END").execute(&server.pool).await.unwrap();
    assert!(events::admit(&server, occurrence(now), now).await.is_err());
    let cursor: Option<String> =
        sqlx::query_scalar("SELECT cursor FROM responsibility_event_cursors")
            .fetch_optional(&server.pool)
            .await
            .unwrap();
    assert_eq!(cursor, None);
    sqlx::query("DROP TRIGGER fail_event")
        .execute(&server.pool)
        .await
        .unwrap();
    assert_eq!(
        events::admit(&server, occurrence(now), now)
            .await
            .unwrap()
            .admitted,
        1
    );
    assert!(
        events::admit(&server, occurrence(now), now)
            .await
            .unwrap()
            .duplicate
    );
}

#[tokio::test]
async fn unavailable_model_blocks_a_due_plan_before_any_provider_request() {
    let home = tempfile::tempdir().unwrap();
    let server = test_server(&home).await;
    let now = Utc::now();
    register(
        &server,
        serde_json::from_value(json!({
            "id":"model-draft","purpose":"Produce a private draft",
            "expiresAt":(now+chrono::Duration::hours(1)).to_rfc3339(),
            "budget":{"tokens":500},
            "plan":{
                "trigger":{"kind":"at","at":now.to_rfc3339()},
                "action":{"kind":"model","version":1,"config":{"text":"Summarize"}},
                "delivery":{"kind":"draft"}
            }
        }))
        .unwrap(),
        now,
    )
    .await
    .unwrap();
    tick(&server, now).await.unwrap();
    let work = jeeves_state::responsibility_runs::take(&server.pool, now.timestamp_millis())
        .await
        .unwrap()
        .unwrap();
    let (sink, _rx) = crate::EventSink::channel();
    worker::process(&server, &sink, &work).await.unwrap();
    let runs = jeeves_state::responsibility_runs::list(&server.pool, "model-draft")
        .await
        .unwrap();
    assert_eq!(runs[0].status, "blocked");
    assert!(runs[0].detail.contains("Set up a model"));
    let requests: i64 = sqlx::query_scalar("SELECT count(*) FROM model_requests")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(requests, 0);
}

#[tokio::test]
async fn scheduled_records_check_the_latest_signed_observation() {
    let home = tempfile::tempdir().unwrap();
    let server = test_server(&home).await;
    let now = Utc::now();
    std::fs::write(
        home.path().join("webhooks.json"),
        json!({"hooks":[{"name":"observations","event_type":"reading","signature":true,
            "event_fields":{"totalMinor":"integer","reference":"string"}}]})
        .to_string(),
    )
    .unwrap();
    let tool = reviews::tool(
        server.clone(),
        "review-snapshot".into(),
        "Check the latest observation every minute and tell me when it is below 1800".into(),
    );
    let plan = json!({
        "trigger":{"kind":"every","intervalSeconds":60},
        "action":{"kind":"records.evaluate","version":1,"config":{
            "all":[{"field":"totalMinor","op":"lt","value":180000}],
            "include":["reference"],"message":"A reading is below the limit",
            "snapshot":{"source":"observations","eventType":"reading","maxAgeSeconds":180}
        }},
        "delivery":{"kind":"chat"}
    });
    let ctx = server.tool_ctx(crate::MAIN_SESSION, None, None);
    (tool.handler)(
        ctx,
        json!({"action":"create","id":"check-reading","fields":{"purpose":"Check a reading","plan":plan}}),
    )
    .await
    .unwrap();
    reviews::decide(
        &server,
        ResponsibilityDecision {
            id: "review-snapshot".into(),
            accept: true,
        },
    )
    .await
    .unwrap();
    let (sink, _rx) = crate::EventSink::channel();
    for (event_id, previous_cursor, value, expected) in [
        ("at-limit", None, 180000, "skipped"),
        ("below-limit", Some("at-limit"), 179999, "confirmed"),
    ] {
        let event = serde_json::from_value(json!({
            "source":"observations","eventType":"reading","eventId":event_id,
            "cursor":event_id,"previousCursor":previous_cursor,"occurredAt":now.to_rfc3339(),
            "fields":{"totalMinor":value,"reference":"signed-receipt"}
        }))
        .unwrap();
        let event_receipt = events::admit(&server, event, now).await.unwrap();
        assert_eq!(event_receipt.admitted, 0);
        let due: i64 =
            sqlx::query_scalar("SELECT next_due_ms FROM responsibilities WHERE id='check-reading'")
                .fetch_one(&server.pool)
                .await
                .unwrap();
        tick(&server, DateTime::from_timestamp_millis(due).unwrap())
            .await
            .unwrap();
        let work = jeeves_state::responsibility_runs::take(&server.pool, due)
            .await
            .unwrap()
            .unwrap();
        worker::process(&server, &sink, &work).await.unwrap();
        let result = worker::reports(&server, "check-reading").await.unwrap();
        let run = result.reports.first().unwrap();
        assert_eq!(run.status, expected);
        if expected == "confirmed" {
            let body = run.body.as_deref().unwrap();
            assert!(
                body.contains("signed-receipt")
                    && body.contains("observations/reading/below-limit")
            );
        }
    }
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM model_requests")
        .fetch_one(&server.pool)
        .await
        .unwrap();
    assert_eq!(calls, 0);
}

#[test]
fn the_catalog_exposes_fetch_and_read_tools_without_hardcoded_sources() {
    let catalog = super::action::catalog();
    let kinds: Vec<&str> = catalog
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"fetch"), "generic fetch must be available");
    assert!(
        !kinds.contains(&"morningBriefing") && !kinds.contains(&"productQuote"),
        "bundled duties must stay out of the action catalog"
    );
}

#[test]
fn digest_accepts_generic_fetch_sources_alongside_named_ones() {
    let config = json!({
        "sources":[
            {"label":"Recent mail","tool":"email_read","args":{"limit":5}}
        ],
        "options":{"timeZone":"Asia/Shanghai","calendarWindow":"today","layout":"list"}
    });
    let adapter = super::action::find(&jeeves_protocol::ResponsibilityAction {
        kind: "digest".into(),
        version: 1,
        config: config.clone(),
    })
    .unwrap();
    let details = (adapter.describe)(&config).unwrap();
    assert!(details.iter().any(|line| line.contains("Recent mail")));
}

#[test]
fn records_accept_a_generic_fetch_source_as_its_record() {
    let with_source = json!({
        "all":[{"field":"price","op":"lt","value":100}],
        "include":["price"],
        "message":"Below target",
        "source":{"tool":"web_search","args":{"query":"price"}}
    });
    let details = (super::records::ADAPTER.describe)(&with_source).unwrap();
    assert!(details.iter().any(|line| line.contains("web_search")));

    let both_sources = json!({
        "all":[{"field":"price","op":"lt","value":100}],
        "include":["price"],
        "message":"Below target",
        "source":{"tool":"web_search","args":{"query":"price"}},
        "snapshot":{"source":"inbox","eventType":"received","maxAgeSeconds":3600}
    });
    assert!(
        (super::records::ADAPTER.describe)(&both_sources).is_err(),
        "mutually exclusive sources must be rejected"
    );
}

#[test]
fn a_cron_slot_is_on_the_minute_regardless_of_approval_time() {
    let approved: DateTime<Utc> = "2030-08-06T00:17:34.567Z".parse().unwrap();
    let expected: DateTime<Utc> = "2030-08-06T07:00:00Z".parse().unwrap();
    let trigger = jeeves_protocol::ResponsibilityTrigger::Cron {
        expression: "0 15 * * *".into(),
        time_zone: "Asia/Shanghai".into(),
    };
    assert_eq!(
        trigger::first(&trigger, approved).unwrap(),
        Some(expected.timestamp_millis())
    );
}
