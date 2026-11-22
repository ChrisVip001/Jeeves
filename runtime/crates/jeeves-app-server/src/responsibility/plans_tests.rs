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
