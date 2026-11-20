use super::input_wake_tests::{fixture, rpc, wait_status};
use serde_json::json;

fn wake(key: &str, source: &str, event_key: &str) -> serde_json::Value {
    json!({"sourceKey": key, "source": source, "eventKey": event_key, "data": {"value": 42}})
}

#[tokio::test]
async fn a_matching_message_wakes_the_waiting_task_once() {
    let (_home, server, _model) = fixture().await;
    rpc(
        &server,
        "task/start",
        json!({"taskId":"input-task","message":"Prepare the meeting report"}),
    )
    .await
    .unwrap();
    wait_status(&server, jeeves_state::TaskStatus::Suspended).await;
    let first = rpc(
        &server,
        "task/wake",
        wake("lark:m1", "approved-host", "figures"),
    )
    .await
    .unwrap();
    assert_eq!(first["woken"], true, "{first}");
    assert_eq!(first["taskId"], "input-task");
    let again = rpc(
        &server,
        "task/wake",
        wake("lark:m1", "approved-host", "figures"),
    )
    .await
    .unwrap();
    assert_eq!(again["woken"], false);
    assert_eq!(again["duplicate"], true);
    wait_status(&server, jeeves_state::TaskStatus::Done).await;
    assert_eq!(server.runs.count_by_task("input-task").await.unwrap(), 1);
}

#[tokio::test]
async fn a_message_nobody_waits_for_is_not_woken() {
    let (_home, server, _model) = fixture().await;
    let other = rpc(&server, "task/wake", wake("lark:m2", "lark", "oc_other"))
        .await
        .unwrap();
    assert_eq!(other["woken"], false);
    assert_eq!(other["duplicate"], false);
    assert!(other.get("taskId").is_none());
}
