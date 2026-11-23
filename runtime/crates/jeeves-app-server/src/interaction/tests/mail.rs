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

#[tokio::test]
async fn main_mail_request_writes_one_local_draft_without_creating_a_task() {
    let (home, server, model) = setup_with_settings(jeeves_config::Settings {
        exec_enabled: true,
        ..Default::default()
    })
    .await;
    configure_mail(home.path());
    send(&server, INSTRUCTION, "draft", None).await.unwrap();
    {
        let received = model.calls.lock().unwrap();
        for tool in ["task", "email_send", "write_file"] {
            assert!(received[0].tools.iter().any(|t| t.name == tool));
        }
        assert!(
            received[0]
                .system_text()
                .unwrap()
                .contains("Deliver the requested result here")
        );
    }
    send(&server, INSTRUCTION, "draft", None).await.unwrap();
    assert!(server.tasks.list(10).await.unwrap().is_empty());
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
async fn deferred_main_discovery_exposes_the_discovered_mail_tool() {
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
    assert!(
        received
            .iter()
            .all(|r| r.tools.iter().any(|t| t.name == "task"))
    );
    assert!(!received[0].tools.iter().any(|t| t.name == "email_send"));
    assert!(received[2].tools.iter().any(|t| t.name == "email_send"));
}
