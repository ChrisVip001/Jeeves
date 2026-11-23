use crate::{EventSink, Frame, Server};
use jeeves_protocol::{
    PermissionMode, RememberScope, ServerNotification, SessionPermissionSetParams,
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn fixture() -> (tempfile::TempDir, Server, Arc<AtomicUsize>) {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    let settings = jeeves_config::Settings {
        home: dir.path().into(),
        ..Default::default()
    };
    let mut server = Server::boot(pool, settings).await;
    server.chat.create_session("s", None, None).await.unwrap();
    let called = Arc::new(AtomicUsize::new(0));
    let observed = called.clone();
    Arc::make_mut(&mut server.tools).register(jeeves_tools::Tool {
        name: "fixture_mutation".into(),
        description: "Unknown effect".into(),
        input_schema: json!({"type":"object"}),
        handler: Arc::new(move |_, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok("changed".into()) })
        }),
    });
    (dir, server, called)
}
async fn mode(
    server: &Server,
    mode: PermissionMode,
    revision: i32,
    confirm: bool,
) -> Result<jeeves_protocol::SessionPermissionResponse, jeeves_protocol::ErrorObject> {
    let (sink, _) = EventSink::channel();
    server
        .set_permission(
            SessionPermissionSetParams {
                session_id: "s".into(),
                mode,
                expected_revision: revision,
                confirm_full_access: confirm,
            },
            &sink,
        )
        .await
}

#[tokio::test]
async fn readonly_is_enforced_at_registry_even_for_unclassified_tools() {
    let (_dir, server, called) = fixture().await;
    mode(&server, PermissionMode::ReadOnly, 0, false)
        .await
        .unwrap();
    let ctx = server.tool_ctx("s", None, None);
    let output = server
        .tools
        .execute(ctx.clone(), "fixture_mutation", json!({}))
        .await;
    assert!(output.starts_with("Error: Read-only"), "{output}");
    let output = server
        .tools
        .execute(
            ctx.clone(),
            "memory",
            json!({"action": "save", "subject":"test","content":"not saved"}),
        )
        .await;
    assert!(output.starts_with("Error: Read-only"), "{output}");
    assert!(
        !server
            .tools
            .execute(ctx, "memory", json!({"action": "list"}))
            .await
            .starts_with("Error:")
    );
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn full_access_requires_confirmation_and_survives_restart_without_enabling_tools() {
    let (_dir, server, called) = fixture().await;
    assert!(
        mode(&server, PermissionMode::FullAccess, 0, false)
            .await
            .is_err()
    );
    let view = mode(&server, PermissionMode::FullAccess, 0, true)
        .await
        .unwrap();
    assert_eq!(view.revision, 1);
    assert!(!view.exec_enabled && !view.write_enabled);
    let visible = crate::enabled_tools(&server.tools, &server.settings());
    assert!(visible.get("run_command").is_none());
    assert!(visible.get("write_file").is_none());
    let ctx = server.tool_ctx("s", None, None);
    assert_eq!(
        server
            .tools
            .execute(ctx, "fixture_mutation", json!({}))
            .await,
        "changed"
    );
    assert_eq!(called.load(Ordering::SeqCst), 1);
    let restored = super::permissions::Policies::load(server.pool.clone()).await;
    assert!(
        jeeves_tools::permissions::ExecutionPolicy::validate(restored.as_ref(), "s", 1).is_ok()
    );
    assert!(
        mode(&server, PermissionMode::ReadOnly, 0, false)
            .await
            .is_err()
    );
    assert_eq!(
        server.permission_view("s").await.unwrap().mode,
        Some(PermissionMode::FullAccess)
    );
}

#[tokio::test]
async fn approval_cannot_execute_after_policy_changes_and_cannot_be_reused() {
    let (_dir, server, called) = fixture().await;
    mode(&server, PermissionMode::Standard, 0, false)
        .await
        .unwrap();
    let (sink, mut rx) = EventSink::channel();
    let ctx = server.tool_ctx("s", None, server.session_approval("s", "t", &sink));
    let tools = server.tools.clone();
    let action =
        tokio::spawn(async move { tools.execute(ctx, "fixture_mutation", json!({})).await });
    let ask = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let Frame::Notification(ServerNotification::ApprovalRequested(ask)) = frame {
            break ask;
        }
    };
    assert_eq!(called.load(Ordering::SeqCst), 0);
    mode(&server, PermissionMode::ReadOnly, 1, false)
        .await
        .unwrap();
    assert!(server.answer_approval("t", &ask.request_id, true, RememberScope::Once));
    assert!(!server.answer_approval("t", &ask.request_id, true, RememberScope::Once));
    assert!(action.await.unwrap().contains("permissions changed"));
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn standard_approval_runs_once_and_revoked_full_broker_cannot_autoapprove() {
    let (_dir, server, called) = fixture().await;
    mode(&server, PermissionMode::FullAccess, 0, true)
        .await
        .unwrap();
    let (sink, mut rx) = EventSink::channel();
    let broker = server.session_approval("s", "t", &sink);
    mode(&server, PermissionMode::Standard, 1, false)
        .await
        .unwrap();
    let ctx = server.tool_ctx("s", None, broker);
    let tools = server.tools.clone();
    let action =
        tokio::spawn(async move { tools.execute(ctx, "fixture_mutation", json!({})).await });
    let ask = loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if let Frame::Notification(ServerNotification::ApprovalRequested(ask)) = frame {
            break ask;
        }
    };
    assert_eq!(called.load(Ordering::SeqCst), 0);
    assert!(server.answer_approval("t", &ask.request_id, true, RememberScope::Once));
    assert_eq!(action.await.unwrap(), "changed");
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn revocation_while_waiting_for_resources_prevents_handler() {
    use jeeves_tools::resources::{Acquire, ResourceCoordinator, ResourceLease};
    struct Barrier {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }
    struct Permit;
    impl ResourceLease for Permit {}
    impl ResourceCoordinator for Barrier {
        fn acquire(&self, _: &str, _: Option<&std::path::Path>) -> Acquire {
            let entered = self.entered.clone();
            let release = self.release.clone();
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
                Ok(Arc::new(Permit) as jeeves_tools::resources::Lease)
            })
        }
    }
    let (_dir, mut server, called) = fixture().await;
    mode(&server, PermissionMode::FullAccess, 0, true)
        .await
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let tools = Arc::make_mut(&mut server.tools);
    tools.set_resources(Arc::new(Barrier {
        entered: entered.clone(),
        release: release.clone(),
    }));
    let ctx = server.tool_ctx("s", None, None);
    let tools = server.tools.clone();
    let action =
        tokio::spawn(async move { tools.execute(ctx, "fixture_mutation", json!({})).await });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    mode(&server, PermissionMode::ReadOnly, 1, false)
        .await
        .unwrap();
    release.notify_one();
    assert!(action.await.unwrap().contains("permissions changed"));
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn capability_switches_are_read_per_turn() {
    let (_dir, server, _) = fixture().await;
    let mut settings = server.settings();
    settings.exec_enabled = false;
    settings.write_enabled = false;
    settings.delegate_enabled = false;
    let off = crate::enabled_tools(&server.tools, &settings);
    for name in [
        "run_command",
        "job",
        "write_file",
        "edit",
        "apply_patch",
        "delegate_task",
    ] {
        assert!(off.get(name).is_none(), "{name} must be hidden while off");
    }
    settings.exec_enabled = true;
    settings.write_enabled = true;
    settings.delegate_enabled = true;
    let on = crate::enabled_tools(&server.tools, &settings);
    for name in [
        "run_command",
        "write_file",
        "edit",
        "apply_patch",
        "delegate_task",
    ] {
        assert!(
            on.get(name).is_some(),
            "{name} must appear once switched on"
        );
    }
}
