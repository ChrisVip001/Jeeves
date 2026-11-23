//! Notifier tests: message format, load semantics, a real Slack POST (loopback listener).

use super::notify::{Notifier, NotifyEvent, Slack, load};
#[test]
fn messages_carry_the_essentials() {
    let approval = NotifyEvent::ApprovalRequested {
        turn_id: "t1".into(),
        tool: "run_command".into(),
        args_preview: "rm -rf /tmp/x".into(),
        reason: "the allowed table did not cover it".into(),
    }
    .message();
    assert!(approval.contains("run_command"), "{approval}");
    assert!(approval.contains("rm -rf"), "{approval}");
    assert!(approval.contains("asks for approval"), "{approval}");

    let job = NotifyEvent::JobFinished {
        id: "j1".into(),
        command: "long-task".into(),
        status: "failed".into(),
        exit_code: Some(2),
    }
    .message();
    assert!(
        job.contains("j1") && job.contains("failed") && job.contains("exit 2"),
        "{job}"
    );

    let job = NotifyEvent::JobFinished {
        id: "j2".into(),
        command: "c".into(),
        status: "interrupted".into(),
        exit_code: None,
    }
    .message();
    assert!(!job.contains("None"), "{job}");

    let goal = NotifyEvent::GoalFinished {
        session_id: "s1".into(),
        status: "round-limit".into(),
        reason: "five continuation rounds and still not there".into(),
    }
    .message();
    assert!(
        goal.contains("round-limit") && goal.contains("rounds"),
        "{goal}"
    );
}

#[test]
fn load_semantics_are_conservative() {
    let dir = tempfile::tempdir().expect("temp dir");
    assert!(
        load(dir.path(), false).is_none(),
        "switched off means nothing"
    );

    assert!(load(dir.path(), true).is_none(), "no file means nothing");

    std::fs::write(dir.path().join("notify.json"), "{}").expect("write an empty object");
    assert!(load(dir.path(), true).is_none(), "no slack field");

    std::fs::write(dir.path().join("notify.json"), r#"{"slack": "  "}"#).expect("write blank");
    assert!(load(dir.path(), true).is_none(), "a blank field");

    std::fs::write(dir.path().join("notify.json"), "{ half a thing")
        .expect("write the broken file");
    assert!(load(dir.path(), true).is_none(), "a bad file is not a bomb");

    std::fs::write(
        dir.path().join("notify.json"),
        r#"{"slack": "https://hooks.slack.com/services/x"}"#,
    )
    .expect("write a good config");
    assert!(
        load(dir.path(), true).is_some(),
        "a config means a notifier"
    );
}

#[tokio::test]
async fn slack_posts_the_text_payload() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("claim a port");
    let port = listener.local_addr().expect("take the port").port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("wait for a connection");
        use std::io::{Read as _, Write as _};
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    });

    let slack = Slack::new(format!("http://127.0.0.1:{port}/hook"));
    slack.notify(NotifyEvent::JobFinished {
        id: "j1".into(),
        command: "echo hello".into(),
        status: "completed".into(),
        exit_code: Some(0),
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    server.join().expect("the service thread");
}

#[tokio::test]
async fn a_dead_endpoint_is_just_a_stderr_line() {
    let slack = Slack::new("http://127.0.0.1:49150/hook".to_string());
    slack.notify(NotifyEvent::GoalFinished {
        session_id: "s1".into(),
        status: "blocked".into(),
        reason: "the judge died".into(),
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}
