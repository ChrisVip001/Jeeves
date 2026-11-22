//! A synthetic owner conversation using the budgeted live provider.
use super::*;

pub(super) async fn run(host: &Server) -> Result<Value> {
    let mut transcript = Vec::new();
    let mut identity = Value::Null;
    let session = jeeves_app_server::MAIN_SESSION;
    for (index, (action, request)) in [
        (
            "create",
            "每周五下午把下周会议和当前未完成任务整理成两段发到这个聊天，带来源。",
        ),
        ("amend", "改到周四，还是上海时间下午三点，只保留会议。"),
        ("pause", "暂停这项周期委托。"),
    ]
    .iter()
    .enumerate()
    {
        let mut message = *request;
        let mut card = Value::Null;
        for attempt in 0..2 {
            rpc(host,"interaction/start",json!({"sessionId":session,"messageId":format!("fixture-{index}-{attempt}"),"message":message})).await?;
            let history = rpc(host, "session/messages", json!({"sessionId":session})).await?;
            let reviews = rpc(host, "responsibility/reviews", json!({})).await?;
            transcript.push(json!({"message":message,"reply":history["data"].as_array().and_then(|a|a.iter().find(|m|m["role"]=="assistant")).map(|m|&m["content"]),"review":reviews[0]}));
            if reviews[0].is_object() {
                card = reviews[0].clone();
                break;
            }
            if index != 0 {
                break;
            }
            message = "用上海时间下午三点。每个来源各一段，在主聊天给我看，先给我核对。";
        }
        let terms = &card["contract"];
        let expected = if index == 0 {
            json!(["calendar", "openTasks"])
        } else {
            json!(["calendar"])
        };
        let sources = terms["sources"].as_array();
        let matched = sources.is_some_and(|actual| {
            actual.len() == expected.as_array().unwrap().len()
                && actual
                    .iter()
                    .all(|s| expected.as_array().unwrap().contains(s))
        });
        let cron = terms["cron"].as_str().unwrap_or_default();
        let valid_schedule = if index == 0 {
            cron.split_whitespace().last() == Some("5")
        } else {
            cron == "0 15 * * 4"
        };
        if card["action"] != *action
            || card["expectedRevision"] != index as i32
            || !matched
            || !valid_schedule
            || terms["digest"]["calendarWindow"] != "nextWeek"
            || terms["digest"]["layout"] != "paragraphs"
            || (index != 0 && identity != terms["id"])
        {
            return Ok(json!({"passed":false,"conversation":transcript,"stage":action}));
        }
        identity = terms["id"].clone();
        rpc(
            host,
            "responsibility/decide",
            json!({"id":card["id"],"accept":true}),
        )
        .await?;
    }
    let result = rpc(host, "responsibility/list", json!({})).await?;
    Ok(
        json!({"passed":result["responsibilities"][0]["status"]=="paused","conversation":transcript,"final":result}),
    )
}
