use super::*;
use crate::responsibility;
use chrono::{DateTime, Utc};
use jeeves_protocol::ResponsibilityDecision;
use jeeves_provider::mock::Mock;
use jeeves_tools::{Tool, ToolEffect};
use tokio::sync::{Barrier, Notify};

#[tokio::test]
async fn main_chat_can_collect_filtered_records_in_reviewed_source_order() {
    check_collection(Scenario::Complete).await;
}

#[tokio::test]
async fn a_malformed_source_keeps_the_collection_out_of_chat() {
    check_collection(Scenario::Malformed).await;
}

#[tokio::test]
async fn a_revoked_read_source_blocks_the_collection_before_tool_calls() {
    check_collection(Scenario::Revoked).await;
}

#[derive(Clone, Copy)]
enum Scenario {
    Complete,
    Malformed,
    Revoked,
}

async fn check_collection(scenario: Scenario) {
    let (_home, mut server, _) = setup().await;
    let both_reading = Arc::new(Barrier::new(2));
    let second_finished = Arc::new(Notify::new());
    let mut tools = (*server.tools).clone();
    tools.register_with_effect(
        Tool {
            name: "fixture_quote_list".into(),
            description: "Read structured fixture quotes".into(),
            input_schema: json!({"type":"object","required":["supplier"],"additionalProperties":false,
                "properties":{"supplier":{"enum":["a","b"]}}}),
            handler: Arc::new(move |_, args| {
                let both_reading = both_reading.clone();
                let second_finished = second_finished.clone();
                Box::pin(async move {
                    tokio::time::timeout(Duration::from_secs(2), both_reading.wait()).await?;
                    if args["supplier"] == "a" {
                        second_finished.notified().await;
                        Ok(json!({"offers":[
                            {"item":"A matched","total":80,"internal":"hidden"},
                            {"item":"A at limit","total":100}
                        ]}).to_string())
                    } else {
                        second_finished.notify_one();
                        Ok(if matches!(scenario, Scenario::Malformed) {
                            json!({"offers":"unavailable"})
                        } else {
                            json!({"offers":[{"item":"B matched","total":90}]})
                        }.to_string())
                    }
                })
            }),
        },
        ToolEffect::Read,
    );
    server.tools = Arc::new(tools);
    let source = |label, supplier| {
        json!({
            "label":label,"read":{"tool":"fixture_quote_list","args":{"supplier":supplier}},
            "rows":"/offers","all":[{"field":"total","op":"lt","value":100}],"include":["item","total"]
        })
    };
    let model = Arc::new(Mock::streaming(vec![
        Mock::tool_use(
            "catalog",
            "recurring_work",
            json!({"action":"capabilities"}),
        ),
        Mock::tool_use(
            "collect",
            "recurring_work",
            json!({
                "action":"create","id":"two-source-report","fields":{"purpose":"Compare two suppliers","plan":{
                    "trigger":{"kind":"after","delaySeconds":1},"delivery":{"kind":"chat"},
                    "action":{"kind":"records.collect","version":1,"config":{"sources":[
                        source("Supplier A", "a"),source("Supplier B", "b")
                    ]}}
                }}
            }),
        ),
        Mock::text("请审阅两处报价来源和筛选条件。"),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 32000,
    });
    send(
        &server,
        "请把两家供应商低于100元的报价汇总到这里，先让我确认。",
        "collect-quotes",
        None,
    )
    .await
    .unwrap();
    let mut reviews = responsibility::reviews::list(&server).await.unwrap();
    assert_eq!(reviews.len(), 1);
    let review = reviews.remove(0);
    let card = review.presentation.unwrap();
    assert_eq!(card.blocked, None);
    assert!(!card.uses_model);
    assert!(
        card.details
            .iter()
            .any(|line| line.contains(r#"Supplier B: fixture_quote_list {"supplier":"b"}"#))
    );
    assert!(
        card.details
            .contains(&"Supplier A: total less than 100".into())
    );
    assert!(
        responsibility::list(&server, Utc::now())
            .await
            .unwrap()
            .responsibilities
            .is_empty()
    );
    let preview = if matches!(scenario, Scenario::Complete) {
        let preview = responsibility::sample(&server, review.contract.clone(), Utc::now())
            .await
            .unwrap();
        assert!(
            responsibility::worker::reports(&server, "two-source-report")
                .await
                .unwrap()
                .reports
                .is_empty()
        );
        Some(preview)
    } else {
        None
    };
    responsibility::reviews::decide(
        &server,
        ResponsibilityDecision {
            id: review.id,
            accept: true,
        },
    )
    .await
    .unwrap();
    if matches!(scenario, Scenario::Revoked) {
        server.tools = Arc::new(server.tools.without("fixture_quote_list"));
    }
    let due =
        sqlx::query_scalar("SELECT next_due_ms FROM responsibilities WHERE id='two-source-report'")
            .fetch_one(&server.pool)
            .await
            .unwrap();
    let at = DateTime::from_timestamp_millis(due).unwrap();
    responsibility::tick(&server, at).await.unwrap();
    let work = jeeves_state::responsibility_runs::take(&server.pool, due)
        .await
        .unwrap()
        .unwrap();
    let (sink, _rx) = EventSink::channel();
    let requests = model.received.lock().unwrap().len();
    responsibility::worker::process(&server, &sink, &work)
        .await
        .unwrap();
    assert_eq!(model.received.lock().unwrap().len(), requests);
    let report = responsibility::worker::reports(&server, "two-source-report")
        .await
        .unwrap()
        .reports
        .remove(0);
    let body: Option<String> = sqlx::query_scalar("SELECT content FROM chat_log WHERE source=?")
        .bind(&work.wake_key)
        .fetch_optional(&server.pool)
        .await
        .unwrap();
    match scenario {
        Scenario::Complete => {
            assert_eq!(report.status, "confirmed");
            assert_eq!(report.tools_used, 2);
            assert_eq!(body, preview);
            assert_eq!(
                body.unwrap(),
                "Supplier A\nSource: fixture_quote_list\n- item: A matched · total: 80\n\nSupplier B\nSource: fixture_quote_list\n- item: B matched · total: 90"
            );
        }
        Scenario::Malformed => {
            assert_eq!(report.status, "review");
            assert_eq!(report.tools_used, 2);
            assert!(report.detail.contains("Supplier B"));
            assert!(report.detail.contains("records array"));
            assert!(body.is_none());
        }
        Scenario::Revoked => {
            assert_eq!(report.status, "blocked");
            assert_eq!(report.tools_used, 0);
            assert!(report.detail.contains("fixture_quote_list"));
            assert!(body.is_none());
        }
    }
    responsibility::tick(&server, at).await.unwrap();
    assert!(
        jeeves_state::responsibility_runs::take(&server.pool, due)
            .await
            .unwrap()
            .is_none()
    );
}
