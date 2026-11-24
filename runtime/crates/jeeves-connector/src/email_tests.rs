use super::*;
use jeeves_tools::approval::{ApprovalBroker, ApprovalFut, ApprovalOutcome};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn config(imap_port: u16, smtp_port: u16) -> EmailConfig {
    serde_json::from_value(json!({
        "username":"alice@example.com",
        "password":"env:JEEVES_TEST_MAIL_PASSWORD",
        "from":"Alice <alice@example.com>",
        "imap":{"host":"127.0.0.1","port":imap_port,"security":"plain"},
        "smtp":{"host":"127.0.0.1","port":smtp_port,"security":"plain"},
        "allow_recipients":["bob@example.com"],
        "live":true
    }))
    .unwrap()
}

async fn ctx() -> ToolCtx {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let pool = jeeves_state::open(&home.join("state.db")).await.unwrap();
    let _ = dir.keep();
    ToolCtx {
        on_uncertain: None,
        resource_lease: None,
        worktree: None,
        task_run_id: None,
        invocation_id: None,
        edit_checks: None,
        activate: None,
        cwd: None,
        approval: None,
        jobs: None,
        turn_limits: Arc::new(jeeves_tools::web::TurnLimits::default()),
        session_id: "test".into(),
        facts: jeeves_state::Facts::new(pool.clone()),
        episodes: jeeves_state::Episodes::new(pool.clone()),
        chat: jeeves_state::Chat::new(pool),
        home,
        on_delta: None,
    }
}

struct Yes;
impl ApprovalBroker for Yes {
    fn request(&self, _: ApprovalRequest) -> ApprovalFut {
        Box::pin(async { ApprovalOutcome::Approved })
    }
}
struct No;
impl ApprovalBroker for No {
    fn request(&self, _: ApprovalRequest) -> ApprovalFut {
        Box::pin(async { ApprovalOutcome::Denied })
    }
}

#[tokio::test]
async fn imap_tls_handshake_failure_is_an_error_without_a_global_crypto_provider() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"not a TLS server\r\n").await.unwrap();
    });
    let tcp = tokio::net::TcpStream::connect(address).await.unwrap();
    let result = tls_connect("localhost", tcp).await;
    assert!(result.is_err(), "plaintext must not be accepted as TLS");
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("IMAP TLS handshake failed")
    );
    server.await.unwrap();
}

#[test]
fn smtp_tls_and_starttls_transport_build_without_a_global_crypto_provider() {
    let mut config = config(993, 465);
    config.smtp.host = "smtp.example.invalid".into();
    for security in [MailSecurity::Tls, MailSecurity::Starttls] {
        config.smtp.security = security;
        assert!(smtp_transport(&config, "fixture-password").is_ok());
    }
}

async fn fake_imap() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"* OK fake imap ready\r\n").await.unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            let tag = line.split_whitespace().next().unwrap_or("*").to_string();
            let upper = line.to_ascii_uppercase();
            if upper.contains("LOGIN") {
                writer
                    .write_all(format!("{tag} OK logged in\r\n").as_bytes())
                    .await
                    .unwrap();
            } else if upper.contains("EXAMINE") {
                writer.write_all(format!("* 1 EXISTS\r\n* OK [UIDVALIDITY 45] valid\r\n* OK [UIDNEXT 8] next\r\n{tag} OK examined\r\n").as_bytes()).await.unwrap();
            } else if upper.contains("SELECT") {
                writer
                    .write_all(format!("* 1 EXISTS\r\n{tag} OK selected\r\n").as_bytes())
                    .await
                    .unwrap();
            } else if upper.contains("UID SEARCH") {
                writer
                    .write_all(format!("* SEARCH 7\r\n{tag} OK search\r\n").as_bytes())
                    .await
                    .unwrap();
            } else if upper.contains("UID FETCH") {
                let raw = b"From: Carol <carol@example.com>\r\nSubject: Hello\r\n\r\nA useful message\r\n";
                writer
                    .write_all(format!("* 1 FETCH (UID 7 BODY[] {{{}}}\r\n", raw.len()).as_bytes())
                    .await
                    .unwrap();
                writer.write_all(raw).await.unwrap();
                writer.write_all(b")\r\n").await.unwrap();
                writer
                    .write_all(format!("{tag} OK fetched\r\n").as_bytes())
                    .await
                    .unwrap();
            } else if upper.contains("LOGOUT") {
                writer.write_all(b"* BYE\r\n").await.unwrap();
                writer
                    .write_all(format!("{tag} OK bye\r\n").as_bytes())
                    .await
                    .unwrap();
                break;
            } else {
                writer
                    .write_all(format!("{tag} OK\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        }
    });
    port
}

async fn fake_smtp(
    accepted: Arc<AtomicBool>,
    data: Arc<Mutex<String>>,
    final_reply: Option<&'static [u8]>,
) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer.write_all(b"220 fake smtp ready\r\n").await.unwrap();
        while let Ok(Some(line)) = lines.next_line().await {
            let upper = line.to_ascii_uppercase();
            if upper.starts_with("EHLO") {
                writer
                    .write_all(b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n")
                    .await
                    .unwrap();
            } else if upper.starts_with("AUTH") {
                writer.write_all(b"235 authenticated\r\n").await.unwrap();
            } else if upper.starts_with("MAIL FROM") || upper.starts_with("RCPT TO") {
                writer.write_all(b"250 ok\r\n").await.unwrap();
            } else if upper == "DATA" {
                writer.write_all(b"354 send it\r\n").await.unwrap();
                let mut body = String::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line == "." {
                        break;
                    }
                    body.push_str(&line);
                    body.push('\n');
                }
                *data.lock().unwrap() = body;
                accepted.store(true, Ordering::SeqCst);
                let Some(reply) = final_reply else { return };
                writer.write_all(reply).await.unwrap();
            } else if upper == "QUIT" {
                writer.write_all(b"221 bye\r\n").await.unwrap();
                break;
            } else {
                writer.write_all(b"250 ok\r\n").await.unwrap();
            }
        }
    });
    port
}

#[tokio::test]
async fn imap_reading_uses_peek_and_returns_plain_text() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let port = fake_imap().await;
    let ctx = ctx().await;
    let out = read(ctx, config(port, 1), json!({"limit":5}))
        .await
        .unwrap();
    assert!(out.contains("A useful message"), "{out}");
    assert!(out.contains("Hello"), "{out}");
}

#[tokio::test]
async fn mailbox_poll_baselines_and_does_not_report_old_uids() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let home = tempfile::tempdir().unwrap();
    let first = poll_inbox(home.path(), &config(fake_imap().await, 1), None)
        .await
        .unwrap();
    assert_eq!(
        (first.uid_validity, first.next_uid, first.unread_uid),
        (45, 8, None)
    );
    let old = poll_inbox(home.path(), &config(fake_imap().await, 1), Some((45, 8)))
        .await
        .unwrap();
    assert_eq!(old.unread_uid, None);
    let new = poll_inbox(home.path(), &config(fake_imap().await, 1), Some((45, 7)))
        .await
        .unwrap();
    assert_eq!(new.unread_uid, Some(7));
    assert!(
        poll_inbox(home.path(), &config(fake_imap().await, 1), Some((46, 7)))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_live_send_is_approved_and_recorded_by_smtp_acceptance() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let accepted = Arc::new(AtomicBool::new(false));
    let data = Arc::new(Mutex::new(String::new()));
    let port = fake_smtp(accepted.clone(), data.clone(), Some(b"250 queued\r\n")).await;
    let mut ctx = ctx().await;
    ctx.approval = Some(Arc::new(Yes));
    let out = send(
        ctx,
        config(1, port),
        json!({"to":["bob@example.com"],"subject":"Report","body":"The numbers are ready"}),
    )
    .await
    .unwrap();
    assert!(out.contains("Standalone SMTP outcome recorded"), "{out}");
    assert!(accepted.load(Ordering::SeqCst));
    let data = data.lock().unwrap();
    assert!(data.contains("Report"), "{data}");
    assert!(data.contains("The numbers are ready"), "{data}");
}

#[tokio::test]
async fn final_smtp_reply_controls_the_reopened_receipt() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    for (reply, status, expected) in [
        (None, "claimed", "unknown"),
        (Some(&b"451 try later\r\n"[..]), "failed", "451"),
        (Some(&b"550 rejected\r\n"[..]), "failed", "550"),
        (Some(&b"250 accepted\r\n"[..]), "confirmed", "250"),
    ] {
        let body_received = Arc::new(AtomicBool::new(false));
        let port = fake_smtp(body_received.clone(), Arc::default(), reply).await;
        let mut ctx = ctx().await;
        let pool = ctx.chat.pool();
        let db = ctx.home.join("state.db");
        jeeves_state::Tasks::new(pool.clone())
            .create(jeeves_state::NewTask {
                id: "mail",
                session_id: Some("main"),
                title: "Synthetic mail",
                goal_condition: None,
                goal_draft: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
        jeeves_state::Runs::new(pool.clone())
            .start("run", "mail", Some("main"), Some("turn"))
            .await
            .unwrap();
        ctx.task_run_id = Some("run".into());
        ctx.approval = Some(Arc::new(Yes));
        let uncertain = Arc::new(AtomicBool::new(false));
        let flag = uncertain.clone();
        ctx.on_uncertain = Some(Arc::new(move |_| flag.store(true, Ordering::SeqCst)));
        let result = send(
            ctx,
            config(1, port),
            json!({"to":["bob@example.com"],"subject":"fixture","body":"test"}),
        )
        .await;
        assert!(body_received.load(Ordering::SeqCst));
        let report = result.unwrap_or_else(|e| e.to_string());
        assert!(report.contains(expected), "{report}");
        assert_eq!(uncertain.load(Ordering::SeqCst), reply.is_none());
        pool.close().await;
        let reopened = jeeves_state::open(&db).await.unwrap();
        let receipts = jeeves_state::Receipts::new(reopened)
            .list("mail")
            .await
            .unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].status, status);
    }
}

#[tokio::test]
async fn an_existing_grant_skips_the_card_only_for_the_same_recipient_and_content() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let accepted = Arc::new(AtomicBool::new(false));
    let data = Arc::new(Mutex::new(String::new()));
    let port = fake_smtp(accepted.clone(), data, Some(b"250 queued\r\n")).await;
    let ctx = ctx().await;
    let hash = jeeves_state::ledger::digest_of(b"Report\0The numbers are ready");
    let grants = Grants::new(ctx.chat.pool());
    grants
        .add(&jeeves_state::Grant {
            id: "seed".into(),
            account: "alice@example.com".into(),
            resource: "email".into(),
            action: "send".into(),
            destination: "bob@example.com".into(),
            content_hash: Some(hash),
            scope: "day".into(),
            expires_at: Some(Utc::now().timestamp() + 3600),
            revoked_version: 0,
            created_at: Utc::now().timestamp(),
        })
        .await
        .unwrap();
    send(
        ctx.clone(),
        config(1, port),
        json!({"to":["bob@example.com"],"subject":"Report","body":"The numbers are ready"}),
    )
    .await
    .unwrap();
    assert!(accepted.load(Ordering::SeqCst));

    let mismatch = send(
        ctx,
        config(1, 9),
        json!({"to":["bob@example.com"],"subject":"Report","body":"Different numbers"}),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(mismatch.contains("approval channel"), "{mismatch}");
}

#[tokio::test]
async fn denied_allowlisted_and_dry_run_sends_never_reach_smtp() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let mut c = config(1, 9);
    let ctx = ctx().await;
    let denied = send(
        ctx.clone(),
        c.clone(),
        json!({"to":["bob@example.com"],"subject":"s","body":"b"}),
    )
    .await;
    assert!(denied.unwrap_err().to_string().contains("approval channel"));

    let mut yes = ctx.clone();
    yes.approval = Some(Arc::new(No));
    assert!(
        send(
            yes,
            c.clone(),
            json!({"to":["bob@example.com"],"subject":"s","body":"b"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("The user denied this email send")
    );

    assert!(
        send(
            ctx.clone(),
            c.clone(),
            json!({"to":["eve@example.com"],"subject":"s","body":"b"})
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("allowlist")
    );

    c.live = false;
    let draft = send(
        ctx,
        c,
        json!({"to":["bob@example.com"],"subject":"s","body":"b"}),
    )
    .await
    .unwrap();
    assert!(draft.contains("draft"), "{draft}");
}

#[tokio::test]
async fn standalone_loss_of_final_reply_survives_restart_and_blocks_repetition() {
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let accepted = Arc::new(AtomicBool::new(false));
    let port = fake_smtp(accepted.clone(), Arc::default(), None).await;
    let mut ctx = ctx().await;
    ctx.approval = Some(Arc::new(Yes));
    let db = ctx.home.join("state.db");
    let args = json!({"to":["bob@example.com"],"subject":"standalone fixture","body":"synthetic"});
    let error = send(ctx.clone(), config(1, port), args.clone())
        .await
        .unwrap_err();
    assert!(accepted.load(Ordering::SeqCst));
    assert!(error.to_string().contains("unknown"));
    ctx.chat.pool().close().await;
    let reopened = jeeves_state::open(&db).await.unwrap();
    let statuses: Vec<String> = sqlx::query_scalar("SELECT state FROM mail_actions")
        .fetch_all(&reopened)
        .await
        .unwrap();
    assert_eq!(statuses, vec!["unknown"]);
    let mut retry = ctx;
    retry.chat = jeeves_state::Chat::new(reopened);
    let blocked = send(retry, config(1, 9), args)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        blocked.contains("no new dispatch") && blocked.contains("unknown"),
        "{blocked}"
    );
}

#[tokio::test]
async fn a_late_approval_cannot_send_a_task_envelope_converted_to_draft() {
    struct HeldApproval {
        asked: Arc<tokio::sync::Notify>,
        answer: Arc<tokio::sync::Notify>,
    }
    impl ApprovalBroker for HeldApproval {
        fn request(&self, _: ApprovalRequest) -> ApprovalFut {
            let (asked, answer) = (self.asked.clone(), self.answer.clone());
            Box::pin(async move {
                asked.notify_one();
                answer.notified().await;
                ApprovalOutcome::Approved
            })
        }
    }
    unsafe {
        std::env::set_var("JEEVES_TEST_MAIL_PASSWORD", "test-secret");
    }
    let mut ctx = ctx().await;
    let pool = ctx.chat.pool();
    jeeves_state::Tasks::new(pool.clone())
        .create(jeeves_state::NewTask {
            id: "mail-task",
            session_id: Some("owner"),
            title: "Fixture memo",
            goal_condition: None,
            goal_draft: None,
            acceptance_json: "[]",
            budget_json: "{}",
            cwd: None,
            max_rounds: 1,
            tier: None,
        })
        .await
        .unwrap();
    jeeves_state::Runs::new(pool.clone())
        .start("mail-run", "mail-task", None, None)
        .await
        .unwrap();
    ctx.task_run_id = Some("mail-run".into());
    let asked = Arc::new(tokio::sync::Notify::new());
    let answer = Arc::new(tokio::sync::Notify::new());
    ctx.approval = Some(Arc::new(HeldApproval {
        asked: asked.clone(),
        answer: answer.clone(),
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sender = tokio::spawn(send(
        ctx,
        config(1, port),
        json!({"to":["bob@example.com"],"subject":"fixture","body":"keep this draft"}),
    ));
    tokio::time::timeout(Duration::from_secs(3), asked.notified())
        .await
        .unwrap();
    let reply = jeeves_state::mail_actions::draft(&pool, "owner", Some("mail-task"), "改成草稿")
        .await
        .unwrap();
    assert!(reply.contains("keep this draft"));
    answer.notify_one();
    let report = tokio::time::timeout(Duration::from_secs(3), sender)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err()
        .to_string();
    assert!(
        report.contains("draft") && report.contains("nothing was sent"),
        "{report}"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_err()
    );
}
