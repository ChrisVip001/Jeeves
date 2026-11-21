use super::*;
use jeeves_provider::mock::Mock;
use std::{fs, path::Path};

pub(super) const INSTRUCTION: &str =
    "请给 lin@example.test 起草一封邮件，主题是会议，正文是「周一见。」";

fn configure_mail(home: &Path) {
    let email = json!({
        "username": "owner@example.test", "from": "owner@example.test",
        "password": "env:UNUSED_MAIL_FIXTURE_PASSWORD",
        "imap":{"host":"localhost", "port":993, "security":"tls"},
        "smtp":{"host":"localhost", "port":465, "security":"tls"},
        "allow_recipients":["lin@example.test"], "live":false
    });
    fs::write(
        home.join("connectors.json"),
        json!({"email":email}).to_string(),
    )
    .unwrap();
}

fn assert_main_tools(request: &CreateRequest) {
    let names: Vec<_> = request.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"task"), "{names:?}");
    assert!(names.contains(&"read_file"), "{names:?}");
    assert!(!names.contains(&"email_send"), "{names:?}");
    assert!(!names.contains(&"write_file"), "{names:?}");
    assert_read_actions(request, "memory", &["search", "list"]);
    let prompt = request.system_text().unwrap();
    assert!(
        prompt.contains("Human session policy: standard"),
        "{prompt}"
    );
    assert!(prompt.contains("use task"), "{prompt}");
}

fn assert_read_actions(request: &CreateRequest, name: &str, expected: &[&str]) {
    let tool = request.tools.iter().find(|tool| tool.name == name).unwrap();
    let actions = tool.input_schema["properties"]["action"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(actions.len(), expected.len(), "{actions:?}");
    for action in expected {
        assert!(actions.contains(&json!(action)), "{actions:?}");
    }
}

#[tokio::test]
async fn main_mail_request_reaches_one_task_and_writes_one_local_draft() {
    let (home, server, model) = setup_with_settings(jeeves_config::Settings {
        exec_enabled: true,
        ..Default::default()
    })
    .await;
    configure_mail(home.path());
    send(&server, INSTRUCTION, "draft", None).await.unwrap();
    {
        let received = model.calls.lock().unwrap();
        assert_main_tools(&received[0]);
        assert_read_actions(&received[0], "job", &["list", "output"]);
    }
    send(&server, INSTRUCTION, "draft", None).await.unwrap();
    let rows = server.tasks.list(10).await.unwrap();
    assert_eq!(rows.len(), 1);
    wait(&server, &rows[0].id).await;
    assert_eq!(server.runs.count_by_task(&rows[0].id).await.unwrap(), 1);
    let drafts: Vec<_> = fs::read_dir(home.path().join("outbox"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("email-")
        })
        .collect();
    assert_eq!(drafts.len(), 1);
    let draft: serde_json::Value = serde_json::from_slice(&fs::read(&drafts[0]).unwrap()).unwrap();
    assert_eq!(draft["to"], json!(["lin@example.test"]));
    assert_eq!(draft["subject"], "会议");
    assert_eq!(draft["body"], "周一见。");
}

#[tokio::test]
async fn deferred_main_discovery_keeps_worker_tools_out_of_the_catalogue() {
    let (home, server, _) = setup_with_settings(jeeves_config::Settings {
        tool_policy: "deferred".into(),
        ..Default::default()
    })
    .await;
    configure_mail(home.path());
    let model = Arc::new(Mock::with_outcomes(vec![
        Mock::context_overflow(),
        Ok(Mock::tool_use(
            "search",
            "find_tools",
            json!({"query":"email_send"}),
        )),
        Ok(Mock::text("工具已检查。")),
    ]));
    server.install_provider(Resolved {
        client: model.clone(),
        ..server.resolved().unwrap()
    });
    send(&server, "主聊天可以查到哪些邮件工具？", "discover", None)
        .await
        .unwrap();
    let received = model.received.lock().unwrap();
    assert_eq!(received.len(), 3);
    for request in received.iter() {
        assert_main_tools(request);
    }
}
