use crate::{EventSink, Frame, Server};
use jeeves_protocol::{RememberScope, ServerNotification};
use jeeves_tools::approval::{ActionGrant, ApprovalRequest};
use std::time::Duration;

async fn setup(
    timeout: i64,
) -> (
    tempfile::TempDir,
    Server,
    EventSink,
    tokio::sync::mpsc::UnboundedReceiver<Frame>,
) {
    let home = tempfile::tempdir().unwrap();
    let server = Server::boot(
        jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap(),
        jeeves_config::Settings {
            home: home.path().into(),
            provider: "test-unconfigured".into(),
            approval_timeout_secs: timeout,
            ..Default::default()
        },
    )
    .await;
    server
        .chat
        .create_session("owner", Some("Owner"), None)
        .await
        .unwrap();
    server.begin_turn("turn", "owner").unwrap();
    let (sink, frames) = EventSink::channel();
    (home, server, sink, frames)
}
fn question() -> ApprovalRequest {
    ApprovalRequest {
        tool: "write_file".into(),
        args_preview: "report.txt".into(),
        reason: "Confirm this write".into(),
        timeout_secs: 1,
        remember: None,
        action: None,
    }
}
#[tokio::test]
async fn an_unpolled_approval_cannot_accept_an_answer_after_its_original_deadline() {
    let (_home, server, sink, mut frames) = setup(1).await;
    let broker = server.approval_bridge_forced("turn", &sink).unwrap();
    let waiting = broker.request(question());
    let Some(Frame::Notification(ServerNotification::ApprovalRequested(ask))) = frames.recv().await
    else {
        panic!("approval notification")
    };
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(!server.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
    assert!(!waiting.await);
}

#[tokio::test]
async fn a_new_client_recovers_only_live_owner_requests_and_cannot_answer_twice() {
    let (_home, server, sink, mut frames) = setup(120).await;
    server
        .chat
        .create_session("other", None, None)
        .await
        .unwrap();
    server
        .chat
        .create_session("worker", None, None)
        .await
        .unwrap();
    server.permissions.bind_task("worker", "owner").unwrap();
    server.begin_turn("worker-turn", "worker").unwrap();
    let broker = server.approval_bridge_forced("worker-turn", &sink).unwrap();
    let waiting = broker.request(question());
    let Some(Frame::Notification(ServerNotification::ApprovalRequested(ask))) = frames.recv().await
    else {
        panic!("approval notification")
    };
    assert!(crate::pending_approvals::list(&server, "other")
        .await
        .unwrap()
        .approvals
        .is_empty());
    let restored = crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap();
    assert_eq!(restored.approvals.len(), 1);
    assert_eq!(restored.approvals[0].session_id, "worker");
    assert_eq!(restored.approvals[0].request.request_id, ask.request_id);
    assert!(restored.approvals[0].request.expires_in_ms <= ask.expires_in_ms);
    assert!(server.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
    assert!(!server.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
    assert!(waiting.await);
    assert!(crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
    assert!(crate::pending_approvals::list(&server, "missing")
        .await
        .is_err());
}

#[tokio::test]
async fn timed_out_or_cancelled_requests_are_not_restored_and_time_is_not_extended() {
    let (_home, server, sink, _frames) = setup(1).await;
    let broker = server.approval_bridge_forced("turn", &sink).unwrap();
    let waiting = broker.request(question());
    let initial = crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap();
    assert_eq!(initial.approvals.len(), 1);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let _ = crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert!(crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
    assert!(!waiting.await);
    let waiting = broker.request(question());
    let old = &initial.approvals[0].request;
    assert!(!server.answer_approval(&old.turn_id, &old.request_id, true, RememberScope::Once));
    assert_eq!(
        crate::pending_approvals::list(&server, "owner")
            .await
            .unwrap()
            .approvals
            .len(),
        1
    );
    drop(waiting);
    assert!(crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
}

#[tokio::test]
async fn a_policy_change_voids_the_snapshot_and_a_restart_restores_no_old_question() {
    let (_home, server, sink, _frames) = setup(120).await;
    let broker = server.approval_bridge_forced("turn", &sink).unwrap();
    let waiting = broker.request(question());
    let snapshot = crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap();
    let ask = &snapshot.approvals[0].request;
    server
        .set_permission(
            jeeves_protocol::SessionPermissionSetParams {
                session_id: "owner".into(),
                mode: jeeves_protocol::PermissionMode::ReadOnly,
                expected_revision: 0,
                confirm_full_access: false,
            },
            &sink,
        )
        .await
        .unwrap();
    assert!(crate::pending_approvals::list(&server, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
    assert!(!server.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
    assert!(!waiting.await);
    let restarted = Server::boot(server.pool.clone(), server.settings()).await;
    assert!(crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
    assert!(!restarted.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
}

async fn ask_and_persist(server: &Server, sink: &EventSink) -> tokio::task::JoinHandle<bool> {
    let broker = server.approval_bridge_forced("turn", sink).unwrap();
    let handle = tokio::spawn(broker.request(question()));
    tokio::time::sleep(Duration::from_millis(150)).await;
    handle
}

#[tokio::test]
async fn a_pending_question_survives_a_restart_with_a_newer_version() {
    let (_home, server, sink, _frames) = setup(120).await;
    let _asking = ask_and_persist(&server, &sink).await;

    let restarted = Server::boot(server.pool.clone(), server.settings()).await;
    let listed = crate::pending_approvals::list(&restarted, "turn-owner-missing").await;
    assert!(listed.is_err(), "unknown conversation still errors");
    restarted
        .chat
        .create_session("owner", Some("Owner"), None)
        .await
        .ok();
    let listed = crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap();
    assert_eq!(listed.approvals.len(), 1, "the question is offered again");
    let view = &listed.approvals[0];
    assert!(view.restored);
    assert_eq!(view.request.version, 2, "a restart bumps the version");
    assert_eq!(view.request.tool, "write_file");
}

#[tokio::test]
async fn an_answer_to_an_older_version_is_refused_with_text() {
    let (_home, server, sink, _frames) = setup(120).await;
    let _asking = ask_and_persist(&server, &sink).await;
    let restarted = Server::boot(server.pool.clone(), server.settings()).await;
    restarted
        .chat
        .create_session("owner", Some("Owner"), None)
        .await
        .ok();
    let request_id = crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap()
        .approvals[0]
        .request
        .request_id
        .clone();

    let stale = restarted.answer_approval_checked(
        "turn",
        &request_id,
        true,
        RememberScope::Once,
        Some(1),
        "owner",
    );
    let error = stale.expect_err("version 1 is stale after the restart");
    assert!(error.starts_with("Error:"), "{error}");
    assert!(
        !crate::pending_approvals::list(&restarted, "owner")
            .await
            .unwrap()
            .approvals
            .is_empty(),
        "a refused answer leaves the question in place"
    );

    let fresh = restarted.answer_approval_checked(
        "turn",
        &request_id,
        true,
        RememberScope::Once,
        Some(2),
        "owner",
    );
    assert_eq!(fresh, Ok(true));
    assert!(crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap()
        .approvals
        .is_empty());
}

#[tokio::test]
async fn the_resumed_turn_reuses_the_yes_given_after_the_restart_exactly_once() {
    let (_home, server, sink, _frames) = setup(120).await;
    let _asking = ask_and_persist(&server, &sink).await;
    let restarted = Server::boot(server.pool.clone(), server.settings()).await;
    restarted
        .chat
        .create_session("owner", Some("Owner"), None)
        .await
        .ok();
    restarted.begin_turn("turn", "owner").unwrap();
    let request_id = crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap()
        .approvals[0]
        .request
        .request_id
        .clone();
    restarted
        .answer_approval_checked(
            "turn",
            &request_id,
            true,
            RememberScope::Once,
            Some(2),
            "owner",
        )
        .unwrap();

    let (sink2, _frames2) = EventSink::channel();
    let broker = restarted.approval_bridge_forced("turn", &sink2).unwrap();
    assert!(broker.request(question()).await, "the stored yes is spent");

    let second = restarted.approval_bridge_forced("turn", &sink2).unwrap();
    let waiting =
        tokio::time::timeout(Duration::from_millis(300), second.request(question())).await;
    assert!(
        waiting.is_err(),
        "the yes was one-time: a second ask waits for a human"
    );
}

#[tokio::test]
async fn a_denial_after_a_restart_is_not_turned_into_a_yes() {
    let (_home, server, sink, _frames) = setup(120).await;
    let _asking = ask_and_persist(&server, &sink).await;
    let restarted = Server::boot(server.pool.clone(), server.settings()).await;
    restarted
        .chat
        .create_session("owner", Some("Owner"), None)
        .await
        .ok();
    let request_id = crate::pending_approvals::list(&restarted, "owner")
        .await
        .unwrap()
        .approvals[0]
        .request
        .request_id
        .clone();
    restarted
        .answer_approval_checked(
            "turn",
            &request_id,
            false,
            RememberScope::Once,
            Some(2),
            "owner",
        )
        .unwrap();
    let (sink2, _f) = EventSink::channel();
    let broker = restarted.approval_bridge_forced("turn", &sink2).unwrap();
    let waiting =
        tokio::time::timeout(Duration::from_millis(300), broker.request(question())).await;
    assert!(
        waiting.is_err(),
        "a no leaves nothing behind; the ask waits again"
    );
}

#[tokio::test]
async fn an_approved_action_persists_a_scoped_once_grant_and_spends_it() {
    let (_home, server, sink, mut frames) = setup(120).await;
    let broker = server.approval_bridge_forced("turn", &sink).unwrap();
    let mut request = question();
    request.tool = "email_send".into();
    request.action = Some(ActionGrant {
        account: "alice@example.com".into(),
        resource: "email".into(),
        action: "send".into(),
        destination: "bob@example.com".into(),
        content_hash: Some("abc".into()),
        scope: "once".into(),
        expires_at: Some(chrono::Utc::now().timestamp() + 120),
    });
    let waiting = broker.request(request);
    let Some(Frame::Notification(ServerNotification::ApprovalRequested(ask))) = frames.recv().await
    else {
        panic!("approval notification")
    };
    assert!(server.answer_approval(&ask.turn_id, &ask.request_id, true, RememberScope::Once));
    assert!(waiting.await);
    let row: (String, String, i64) = sqlx::query_as(
        "SELECT scope, destination, revoked_version FROM action_grants WHERE destination = ?",
    )
    .bind("bob@example.com")
    .fetch_one(&server.chat.pool())
    .await
    .unwrap();
    assert_eq!(row, ("once".into(), "bob@example.com".into(), 1));
}
