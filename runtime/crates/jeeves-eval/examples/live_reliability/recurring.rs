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

pub(super) async fn event(host: &Server, home: &std::path::Path) -> Result<Value> {
    std::fs::write(
        home.join("webhooks.json"),
        serde_json::to_vec(&json!({"hooks":[{
            "name":"connected-mailbox","signature":true,"event_type":"mail.received",
            "event_fields":{"sender":"string","attachmentTypes":"stringArray","subject":"string","messageRef":"string"},
        "event_field_examples":{"attachmentTypes":["application/pdf","text/plain","image/png"],"messageRef":"mail://connected-mailbox/msg-001"}
        }]}))?,
    )?;
    let session = jeeves_app_server::MAIN_SESSION;
    let message = "已连接的报销邮箱收到新邮件时，只有发件人是 finance@example.test 并且有 PDF 附件才在主聊天提醒我。附邮件主题和原邮件来源。先给我确认。";
    rpc(
        host,
        "interaction/start",
        json!({"sessionId":session,"messageId":"held-out-mail","message":message}),
    )
    .await?;
    let history = rpc(host, "session/messages", json!({"sessionId":session})).await?;
    let reviews = rpc(host, "responsibility/reviews", json!({})).await?;
    let list = rpc(host, "responsibility/list", json!({})).await?;
    let plan = &reviews[0]["contract"]["plan"];
    let checks = plan["action"]["config"]["all"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let sources = plan["action"]["config"]["include"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let expected = checks.len() == 2
        && checks.iter().any(|row| {
            row["field"] == "sender"
                && ((row["op"] == "in" && row["value"] == json!(["finance@example.test"]))
                    || (row["op"] == "eq" && row["value"] == "finance@example.test"))
        })
        && checks.iter().any(|row| {
            row["field"] == "attachmentTypes"
                && row["op"] == "contains"
                && row["value"] == "application/pdf"
        })
        && sources.contains(&json!("subject"))
        && sources.contains(&json!("messageRef"));
    let passed = plan["trigger"]["source"] == "connected-mailbox"
        && plan["trigger"]["kind"] == "event"
        && plan["trigger"]["eventType"] == "mail.received"
        && plan["delivery"]["kind"] == "chat"
        && plan["action"]["kind"] == "records.evaluate"
        && expected
        && reviews[0]["presentation"]["blocked"].is_null()
        && list["responsibilities"] == json!([]);
    Ok(
        json!({"passed":passed,"request":message,"history":history,"reviews":reviews,"unconfirmed":list}),
    )
}
