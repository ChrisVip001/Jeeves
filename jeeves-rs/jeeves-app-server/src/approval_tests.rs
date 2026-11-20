//! The full approval chain: run all the way to "the command executed inside the sandbox".
//!
//! This deliberately uses a **real Server + real sandbox** (on machines that have one): the value of the
//! approval mechanism lives entirely in the causality "not before being asked, only after being answered" — fake either half and the test cannot detect it.

use std::sync::Arc;

use jeeves_protocol::{RememberScope, RequestId, ServerNotification, TurnStartParams};
use jeeves_provider::mock::Mock;
use jeeves_provider::Resolved;
use jeeves_tools::approval::RememberTarget;
use tokio::sync::oneshot;

use crate::{run_turn, EventSink, Frame, Server};

#[tokio::test]
async fn answering_an_unknown_request_is_false_not_an_error() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    assert!(!server.answer_approval("no-such-turn", "ap-1", true, RememberScope::Once));
}

#[tokio::test]
async fn a_turn_asks_and_runs_the_command_only_after_being_answered() {
    if !jeeves_tools::exec::sandbox_available() {
        return;
    }

    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        provider: "openai".to_string(),
        api_key: Some("test-key".to_string()),
        model: Some("test-model".to_string()),
        small_model: Some("test-model".to_string()),
        exec_enabled: true,
        exec_allow: vec!["ls *".to_string()],
        approval: "on-request".to_string(),
        approval_timeout_secs: 20,
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    server.install_provider(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(Mock::new(vec![
            Mock::tool_use(
                "call-1",
                "run_command",
                serde_json::json!({"command": "echo approved-ok"}),
            ),
            Mock::text("done running."),
        ])),
        model: "test-model".to_string(),
        small_model: "test-model".to_string(),
    });

    let (sink, mut rx) = EventSink::channel();

    let answerer = tokio::spawn({
        let server = server.clone();
        async move {
            let mut asked = None;
            let mut tool_output = None;
            while let Some(frame) = rx.recv().await {
                let Frame::Notification(notification) = frame else {
                    continue;
                };
                match notification {
                    ServerNotification::ApprovalRequested(ask) => {
                        let accepted = server.answer_approval(
                            &ask.turn_id,
                            &ask.request_id,
                            true,
                            RememberScope::Once,
                        );
                        asked = Some((ask.args_preview.clone(), accepted));
                    }
                    ServerNotification::ToolCompleted(done) => {
                        tool_output = Some(done.output.clone());
                    }
                    _ => {}
                }
            }
            (asked, tool_output)
        }
    });

    run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "run echo".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .expect("this turn should finish");

    drop(sink);
    let (asked, tool_output) = answerer.await.expect("the answering task");
    let (preview, accepted) = asked.expect("there should be a request");
    assert_eq!(
        preview, "echo approved-ok",
        "the question must be the command itself"
    );
    assert!(accepted, "the answer arrived within the deadline");

    let output = tool_output.expect("there should be a tool-finished notice");
    assert!(
        output.contains("approved-ok"),
        "after approval the command has to really run in the sandbox: {output}"
    );
    assert!(output.contains("exit code 0"), "{output}");
}

#[tokio::test]
async fn without_on_request_the_command_is_simply_refused() {
    if !jeeves_tools::exec::sandbox_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        provider: "openai".to_string(),
        api_key: Some("test-key".to_string()),
        model: Some("test-model".to_string()),
        small_model: Some("test-model".to_string()),
        exec_enabled: true,
        exec_allow: vec!["ls *".to_string()],
        ..Default::default()
    };
    let server: Server = crate::open(&settings).await.expect("assemble");
    server.install_provider(Resolved {
        context_window: jeeves_provider::DEFAULT_CONTEXT_WINDOW,
        vision: false,
        provider_id: "mock".to_string(),
        client: Arc::new(Mock::new(vec![
            Mock::tool_use(
                "call-1",
                "run_command",
                serde_json::json!({"command": "echo nope"}),
            ),
            Mock::text("fine, it does not run."),
        ])),
        model: "test-model".to_string(),
        small_model: "test-model".to_string(),
    });

    let (sink, mut rx) = EventSink::channel();
    let watcher = tokio::spawn(async move {
        let mut asked = false;
        let mut output = String::new();
        while let Some(frame) = rx.recv().await {
            if let Frame::Notification(notification) = frame {
                match notification {
                    ServerNotification::ApprovalRequested(_) => asked = true,
                    ServerNotification::ToolCompleted(done) => output = done.output.clone(),
                    _ => {}
                }
            }
        }
        (asked, output)
    });

    run_turn(
        &server,
        TurnStartParams {
            request_id: None,
            attachments: None,
            session_id: Some("test".to_string()),
            message: "run it".to_string(),
            stream: Some(true),
            cwd: None,
            tier: None,
        },
        RequestId::Number(1),
        &sink,
    )
    .await
    .expect("this turn should finish");

    drop(sink);
    let (asked, output) = watcher.await.expect("the watching task");
    assert!(!asked, "never mode must not ask anything");
    assert!(output.starts_with("Error:"), "{output}");
    assert!(
        output.contains("No allow rule matched"),
        "the refusal reason has to point at the allowlist: {output}"
    );
}

fn ask(
    server: &Server,
    turn_id: &str,
    request_id: &str,
    target: RememberTarget,
) -> oneshot::Receiver<(bool, String)> {
    let (tx, rx) = oneshot::channel();
    server
        .approvals
        .lock()
        .expect("the approvals lock should not be poisoned")
        .entry(turn_id.to_string())
        .or_default()
        .insert(
            request_id.to_string(),
            crate::approval::Waiting::new(
                tx,
                Some(target),
                jeeves_protocol::ApprovalRequestedNotification {
                    turn_id: turn_id.into(),
                    request_id: request_id.into(),
                    tool: "test".into(),
                    args_preview: String::new(),
                    reason: String::new(),
                    expires_in_ms: 120000,
                    version: 1,
                },
                std::time::Duration::from_secs(120),
            ),
        );
    rx
}

fn settings_json(home: &std::path::Path) -> String {
    std::fs::read_to_string(jeeves_config::patch_path(home)).unwrap_or_default()
}

#[tokio::test]
async fn remembering_a_command_persists_and_takes_effect_at_once() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");

    let rx = ask(
        &server,
        "t1",
        "ap-1",
        RememberTarget::Command("git status".to_string()),
    );
    assert!(server.answer_approval("t1", "ap-1", true, RememberScope::Always));
    assert!(rx.await.expect("the answer has to arrive").0, "approved");

    assert!(
        server
            .exec_session_allow
            .lock()
            .expect("lock")
            .contains(&"git status".to_string()),
        "takes effect in this process at once"
    );
    assert!(
        server
            .settings()
            .exec_allow
            .iter()
            .any(|rule| rule == "git status"),
        "and has to reach the in-memory settings too"
    );
    let raw = settings_json(dir.path());
    assert!(
        raw.contains("git status"),
        "and it lands in settings.json: {raw}"
    );
}

#[tokio::test]
async fn once_and_session_leave_different_traces() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");

    let rx = ask(
        &server,
        "t1",
        "ap-1",
        RememberTarget::Command("make".to_string()),
    );
    assert!(server.answer_approval("t1", "ap-1", true, RememberScope::Once));
    assert!(rx.await.expect("delivered").0);
    assert!(
        !server
            .exec_session_allow
            .lock()
            .expect("lock")
            .contains(&"make".to_string()),
        "once = nothing is remembered"
    );
    assert_eq!(
        settings_json(dir.path()),
        "",
        "a one-off approval writes nothing"
    );

    let rx = ask(
        &server,
        "t2",
        "ap-2",
        RememberTarget::Command("cargo test".to_string()),
    );
    assert!(server.answer_approval("t2", "ap-2", true, RememberScope::Session));
    assert!(rx.await.expect("delivered").0);
    assert!(
        server
            .exec_session_allow
            .lock()
            .expect("lock")
            .contains(&"cargo test".to_string()),
        "remembered for this session"
    );
    assert_eq!(
        settings_json(dir.path()),
        "",
        "session scope writes nothing either"
    );
}

#[tokio::test]
async fn replacing_the_allow_table_revokes_rules_immediately() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        exec_allow: vec!["ls *".to_string(), "pwd".to_string()],
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    assert_eq!(server.exec_allow.lock().expect("lock").len(), 2);
    server
        .apply_config_patch(&jeeves_protocol::SettingsPatch {
            exec_allow: Some(vec!["pwd".to_string()]),
            ..Default::default()
        })
        .expect("a valid patch");
    assert_eq!(
        *server.exec_allow.lock().expect("lock"),
        vec!["pwd".to_string()]
    );
}

#[tokio::test]
async fn the_answer_carries_who_gave_it() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    let rx = ask(
        &server,
        "t1",
        "ap-1",
        RememberTarget::Command("ls".to_string()),
    );
    assert!(server.answer_approval_as("t1", "ap-1", true, RememberScope::Once, "lark:ou_1"));
    assert_eq!(
        rx.await.expect("delivered"),
        (true, "lark:ou_1".to_string())
    );
    let rx = ask(
        &server,
        "t1",
        "ap-2",
        RememberTarget::Command("ls".to_string()),
    );
    assert!(server.answer_approval("t1", "ap-2", false, RememberScope::Once));
    assert_eq!(rx.await.expect("delivered"), (false, "owner".to_string()));
}

#[tokio::test]
async fn a_denial_is_never_remembered() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let settings = jeeves_config::Settings {
        home: dir.path().to_path_buf(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");
    let rx = ask(
        &server,
        "t1",
        "ap-1",
        RememberTarget::Command("rm x".to_string()),
    );
    assert!(server.answer_approval("t1", "ap-1", false, RememberScope::Always));
    assert!(!rx.await.expect("delivered").0, "a refusal is a refusal");
    assert!(server.exec_session_allow.lock().expect("lock").is_empty());
    assert_eq!(settings_json(dir.path()), "");
}

#[tokio::test]
async fn a_remembered_write_directory_needs_no_answer() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let workspace = dir.path().join("ws");
    std::fs::create_dir_all(&workspace).expect("create the workspace");
    let settings = jeeves_config::Settings {
        home: dir.path().join("home"),
        write_enabled: true,
        approval: "on-request".to_string(),
        ..Default::default()
    };
    let server = crate::open(&settings).await.expect("assemble");

    let rx = ask(
        &server,
        "t1",
        "ap-1",
        RememberTarget::Directory(workspace.clone()),
    );
    assert!(server.answer_approval("t1", "ap-1", true, RememberScope::Always));
    assert!(rx.await.expect("delivered").0);

    let ctx = server.tool_ctx("s1", Some(workspace.clone()), None);
    let path = workspace.join("note.md");
    let out = server
        .tools
        .execute(
            ctx,
            "write_file",
            serde_json::json!({"path": path.display().to_string(), "content": "hi\n"}),
        )
        .await;
    assert!(out.starts_with("Wrote"), "{out}");
    assert!(path.exists(), "it really was written");
    let escaped = serde_json::to_string(workspace.to_string_lossy().as_ref())
        .expect("workspace path is valid UTF-8");
    assert!(
        settings_json(&settings.home).contains(escaped.trim_matches('"')),
        "the directory reached settings.json"
    );
}

#[tokio::test]
async fn a_late_answer_is_told_who_answered_first() {
    use crate::input_wake_tests::{fixture, rpc};
    let (_home, server, _model) = fixture().await;
    server
        .approval_log
        .log(&jeeves_state::approvals::Decision {
            turn_id: "t1",
            request_id: "r1",
            tool: "run_command",
            args_preview: "ls",
            reason: "why",
            outcome: "approved",
            actor: "telegram:7",
        })
        .await
        .unwrap();
    let late = rpc(
        &server,
        "approval/respond",
        serde_json::json!({"turnId":"t1","requestId":"r1","approved":true,"remember":"once","actor":"lark:ou_1"}),
    )
    .await
    .unwrap();
    assert_eq!(late["accepted"], false);
    assert_eq!(late["answeredBy"], "telegram:7");
    let unknown = rpc(
        &server,
        "approval/respond",
        serde_json::json!({"turnId":"t9","requestId":"r9","approved":true,"remember":"once"}),
    )
    .await
    .unwrap();
    assert_eq!(unknown["accepted"], false);
    assert!(unknown.get("answeredBy").is_none());
}
