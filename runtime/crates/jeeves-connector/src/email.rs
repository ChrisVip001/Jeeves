//! Email connector: IMAP reads use BODY.PEEK (never mark a human's mail read); SMTP sends are
//! drafts by default, allowlisted, and every live recipient gets its own scoped approval.

use anyhow::{Context, Result, bail};
use async_imap::Client;
use chrono::Utc;
use jeeves_state::{GrantRequest, Grants};
use jeeves_tools::approval::{ActionGrant, ApprovalRequest};
use jeeves_tools::{Tool, ToolCtx, require_str};
use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::{Value, json};
use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::config::{EmailConfig, MailSecurity};

const APPROVAL_TIMEOUT_SECS: i64 = 120;
const MAX_BODY_CHARS: usize = 16_000;

trait MailStream: AsyncRead + AsyncWrite + Unpin + Debug + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send> MailStream for T {}
type BoxStream = Box<dyn MailStream>;

pub fn email_read_tool(config: EmailConfig) -> Tool {
    Tool {
        name: "email_read".into(),
        description: "Read recent UNSEEN mail headers and plain-text bodies over IMAP. Uses BODY.PEEK, so messages stay unread. Read-only.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "mailbox":{"type":"string","description":"mailbox name; defaults to INBOX"},
                "limit":{"type":"integer","minimum":1,"maximum":50}
            },
            "additionalProperties":false
        }),
        handler: Arc::new(move |ctx, args| {
            let config = config.clone();
            Box::pin(async move { read(ctx, config, args).await })
        }),
    }
}

pub fn email_send_tool(config: EmailConfig) -> Tool {
    Tool {
        name: "email_send".into(),
        description: "Draft or send an email. Draft mode is the default; live sends require allowlisting and a scoped human approval per recipient and exact content.".into(),
        input_schema: json!({
            "type":"object",
            "properties":{
                "to":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":20},
                "subject":{"type":"string"},
                "body":{"type":"string"},
                "dry_run":{"type":"boolean","description":"write a draft even when live is on; unset follows the config"}
            },
            "required":["to","subject","body"],
            "additionalProperties":false
        }),
        handler: Arc::new(move |ctx, args| {
            let config = config.clone();
            Box::pin(async move { send(ctx, config, args).await })
        }),
    }
}

async fn read(ctx: ToolCtx, config: EmailConfig, args: Value) -> Result<String> {
    let mailbox = args
        .get("mailbox")
        .and_then(Value::as_str)
        .unwrap_or("INBOX")
        .to_string();
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .min(50) as usize;
    let secret = config
        .secret(&ctx.home)
        .context("the email password was not found at its configured env/keychain/file pointer")?;
    let mut session = imap_session(&config, &secret).await?;
    session
        .select(&mailbox)
        .await
        .context("IMAP SELECT failed")?;
    let found = session
        .uid_search("UNSEEN")
        .await
        .context("IMAP UID SEARCH failed")?;
    let mut uids: Vec<u32> = found.into_iter().collect();
    uids.sort_unstable_by(|a, b| b.cmp(a));
    uids.truncate(limit);
    if uids.is_empty() {
        let _ = session.logout().await;
        return Ok(json!({"mailbox":mailbox,"messages":[],"count":0}).to_string());
    }
    let set = uids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut fetches = session
        .uid_fetch(set, "BODY.PEEK[]")
        .await
        .context("IMAP UID FETCH failed")?;
    let mut messages = Vec::new();
    use futures_util::TryStreamExt;
    while let Some(message) = fetches.try_next().await? {
        let uid = message.uid.unwrap_or_default();
        let Some(raw) = message.body() else { continue };
        let parsed = match mailparse::parse_mail(raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                messages.push(json!({"uid":uid,"error":format!("could not parse MIME: {error}")}));
                continue;
            }
        };
        use mailparse::MailHeaderMap;
        let subject = parsed
            .headers
            .get_first_value("Subject")
            .unwrap_or_else(|| "(no subject)".into());
        let from = parsed
            .headers
            .get_first_value("From")
            .unwrap_or_else(|| "(unknown sender)".into());
        let date = parsed.headers.get_first_value("Date");
        let body = parsed.get_body().unwrap_or_default();
        let body = cap_chars(body, MAX_BODY_CHARS);
        messages.push(json!({"uid":uid,"from":from,"subject":subject,"date":date,"body":body}));
    }
    drop(fetches);
    let _ = session.logout().await;
    Ok(serde_json::to_string_pretty(
        &json!({"mailbox":mailbox,"messages":messages,"count":messages.len()}),
    )?)
}

fn cap_chars(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        text
    } else {
        let mut capped: String = text.chars().take(max).collect();
        capped.push_str("\n[truncated]");
        capped
    }
}

async fn send(ctx: ToolCtx, config: EmailConfig, args: Value) -> Result<String> {
    let recipients = parse_recipients(&args)?;
    let subject = require_str(&args, "subject")?;
    let body = require_str(&args, "body")?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    for recipient in &recipients {
        if !config.permits(recipient.email.as_ref()) {
            bail!(
                "Recipient {recipient} is not on the email allowlist; only a person can edit allow_recipients in connectors.json"
            );
        }
    }
    if dry_run || !config.live {
        let path = write_draft(&ctx.home, &config, &recipients, &subject, &body).await?;
        return Ok(format!(
            "Email draft written to {} for {} recipient(s); nothing was sent{}.",
            path.display(),
            recipients.len(),
            if config.live {
                " because dry_run was set"
            } else {
                ""
            }
        ));
    }

    let secret = config
        .secret(&ctx.home)
        .context("the email password was not found at its configured env/keychain/file pointer")?;
    let message_id = format!(
        "<{}-{}@jeeves.local>",
        Utc::now().timestamp_nanos_opt().unwrap_or_default(),
        std::process::id()
    );
    let from: Mailbox = config
        .from
        .parse()
        .context("email.from is not a valid mailbox")?;
    let mut builder = Message::builder()
        .from(from)
        .subject(&subject)
        .message_id(Some(message_id.clone()));
    for recipient in &recipients {
        builder = builder.to(recipient.clone());
    }
    let message = builder
        .body(body.clone())
        .context("could not build the email")?;
    let mailer = smtp_transport(&config, &secret)?;
    let envelope = json!({"from":config.from,"to":recipients.iter().map(ToString::to_string).collect::<Vec<_>>(),"subject":subject,"body":body}).to_string();
    let (action, fresh) = match ctx.task_run_id.as_deref() {
        Some(run) => {
            jeeves_state::mail_actions::MailAction::queue(
                ctx.chat.pool(),
                run,
                &config.username,
                &envelope,
            )
            .await?
        }
        None => {
            jeeves_state::mail_actions::MailAction::standalone(
                ctx.chat.pool(),
                &ctx.session_id,
                ctx.invocation_id.as_deref().unwrap_or(&message_id),
                &config.username,
                &envelope,
            )
            .await?
        }
    };
    if !fresh {
        bail!(
            "Existing email action {} is {}; no new dispatch",
            action.id,
            action.state().await?
        );
    }
    let approved = match approve(&ctx, &config, &recipients, &subject, &body).await {
        Ok(destinations) => destinations,
        Err(error) => {
            action.deny().await?;
            return Err(error);
        }
    };
    if !action.claim().await? {
        bail!(
            "Email action {} is {}; nothing was sent",
            action.id,
            action.state().await?
        );
    }
    match mailer.send(message).await {
        Ok(response) => {
            action.settle("confirmed").await?;
            let evidence = format!("smtp:{}", response.code());
            let receipt = record_receipt(&ctx, &message_id, true, Some(&evidence)).await;
            Ok(format!(
                "Sent to {} (Message-ID {message_id}, SMTP {}). {receipt}",
                approved.join(", "),
                response.code()
            ))
        }
        Err(error) => {
            if let Some(code) = error.status() {
                action.settle("rejected").await?;
                let evidence = format!("smtp:{code}");
                let receipt = record_receipt(&ctx, &message_id, false, Some(&evidence)).await;
                bail!("SMTP rejected the message ({code}): {error}. {receipt}");
            }
            if let Some(sink) = &ctx.on_uncertain {
                sink(&format!(
                    "email send {message_id} has an uncertain SMTP outcome: {error}"
                ));
            }
            action.settle("unknown").await?;
            record_claimed(&ctx, &message_id).await;
            bail!(
                "SMTP outcome is unknown after the send started; action {} was not retried: {error}",
                action.id
            );
        }
    }
}

async fn approve(
    ctx: &ToolCtx,
    config: &EmailConfig,
    recipients: &[Mailbox],
    subject: &str,
    body: &str,
) -> Result<Vec<String>> {
    let content_hash = jeeves_state::ledger::digest_of(format!("{subject}\0{body}").as_bytes());
    let now = Utc::now().timestamp();
    let grants = Grants::new(ctx.chat.pool());
    let mut approved = Vec::new();
    for recipient in recipients {
        let destination = recipient.email.to_string().to_ascii_lowercase();
        let proof = ActionGrant {
            account: config.username.clone(),
            resource: "email".into(),
            action: "send".into(),
            destination: destination.clone(),
            content_hash: Some(content_hash.clone()),
            scope: "once".into(),
            expires_at: Some(now + APPROVAL_TIMEOUT_SECS),
        };
        let request = GrantRequest {
            account: &proof.account,
            resource: &proof.resource,
            action: &proof.action,
            destination: &proof.destination,
            content_hash: proof.content_hash.as_deref(),
            now,
        };
        if ctx.task_run_id.is_none() && grants.consume(&request).await?.is_some() {
            eprintln!("(jeeves) email send to {destination} used an existing action grant");
            approved.push(destination);
            continue;
        }
        let Some(broker) = ctx.approval.clone() else {
            bail!("Sending email for real needs a human approval channel; nothing was sent");
        };
        let preview = format!(
            "Account {}\nTo {destination}\nSubject: {subject}\nContent digest: {content_hash}\nExpires: {}\nGrant revision: 0\n\n{}",
            config.username,
            proof.expires_at.unwrap_or_default(),
            body
        );
        let outcome = broker
            .request(ApprovalRequest {
                tool: "email_send".into(),
                args_preview: preview,
                reason: "live email send: this recipient, subject and body are authorized together"
                    .into(),
                timeout_secs: APPROVAL_TIMEOUT_SECS,
                remember: None,
                action: Some(proof),
            })
            .await;
        if !outcome.approved() {
            bail!(
                "{} No message was sent to {destination}.",
                outcome.refusal("email send")
            );
        }
        approved.push(destination);
    }

    Ok(approved)
}

fn parse_recipients(args: &Value) -> Result<Vec<Mailbox>> {
    let values = args
        .get("to")
        .and_then(Value::as_array)
        .context("Missing parameter 'to'")?;
    if values.is_empty() || values.len() > 20 {
        bail!("Parameter 'to' must contain between 1 and 20 recipients");
    }
    values
        .iter()
        .map(|value| {
            let raw = value.as_str().context("each recipient must be a string")?;
            raw.parse::<Mailbox>()
                .with_context(|| format!("invalid recipient {raw:?}"))
        })
        .collect()
}

async fn write_draft(
    home: &std::path::Path,
    config: &EmailConfig,
    recipients: &[Mailbox],
    subject: &str,
    body: &str,
) -> Result<std::path::PathBuf> {
    let dir = home.join("outbox");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!(
        "email-{}.json",
        Utc::now().format("%Y%m%dT%H%M%S%.3f")
    ));
    let draft = json!({
        "from":config.from,
        "to":recipients.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "subject":subject,
        "body":body,
        "live":config.live,
    });
    tokio::fs::write(&path, serde_json::to_vec_pretty(&draft)?).await?;
    Ok(path)
}

async fn record_claimed(ctx: &ToolCtx, message_id: &str) {
    let Some((receipts, task_id, run_id)) = receipt_target(ctx).await else {
        return;
    };
    if let Err(error) = receipts
        .claim(
            &task_id,
            Some(&run_id),
            "email_message",
            &format!("email:{message_id}"),
        )
        .await
    {
        eprintln!("(jeeves) the email receipt could not be claimed: {error}");
    }
}

async fn record_receipt(
    ctx: &ToolCtx,
    message_id: &str,
    confirmed: bool,
    evidence: Option<&str>,
) -> String {
    let Some((receipts, task_id, run_id)) = receipt_target(ctx).await else {
        return if ctx.task_run_id.is_none() {
            "Standalone SMTP outcome recorded."
        } else {
            "SMTP action recorded; task receipt unavailable."
        }
        .into();
    };
    match receipts
        .claim(
            &task_id,
            Some(&run_id),
            "email_message",
            &format!("email:{message_id}"),
        )
        .await
    {
        Ok(id) => {
            let settled = if confirmed {
                receipts.confirm(id, evidence).await
            } else {
                receipts.fail(id, evidence).await
            };
            match settled {
                Ok(()) if confirmed => format!("Receipt {id} confirmed by SMTP acceptance."),
                Ok(()) => format!("Receipt {id} recorded as failed."),
                Err(error) => {
                    eprintln!("(jeeves) the email receipt {id} could not be settled: {error}");
                    format!("Receipt {id} could not be settled: {error}.")
                }
            }
        }
        Err(error) => {
            eprintln!("(jeeves) the email receipt could not be claimed: {error}");
            format!("No receipt: {error}")
        }
    }
}

async fn receipt_target(ctx: &ToolCtx) -> Option<(jeeves_state::Receipts, String, String)> {
    let run_id = ctx.task_run_id.as_deref()?;
    let run = jeeves_state::Runs::new(ctx.chat.pool())
        .get(run_id)
        .await
        .ok()??;
    Some((
        jeeves_state::Receipts::new(ctx.chat.pool()),
        run.task_id,
        run_id.to_string(),
    ))
}

fn smtp_transport(
    config: &EmailConfig,
    secret: &str,
) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    let builder = match config.smtp.security {
        MailSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.smtp.host)?,
        MailSecurity::Starttls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.smtp.host)?
        }
        MailSecurity::Plain => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.smtp.host)
        }
    };
    Ok(builder
        .port(config.smtp.port)
        .credentials(lettre::transport::smtp::authentication::Credentials::new(
            config.username.clone(),
            secret.to_string(),
        ))
        .timeout(Some(Duration::from_secs(30)))
        .build())
}

async fn imap_session(
    config: &EmailConfig,
    secret: &str,
) -> Result<async_imap::Session<BoxStream>> {
    let host = config.imap.host.as_str();
    let tcp = tokio::net::TcpStream::connect((host, config.imap.port))
        .await
        .with_context(|| format!("could not connect to IMAP {host}:{}", config.imap.port))?;
    let client = match config.imap.security {
        MailSecurity::Tls => {
            let tls = tls_connect(host, tcp).await?;
            let mut client = Client::new(Box::new(tls) as BoxStream);
            read_greeting(&mut client).await?;
            client
        }
        MailSecurity::Starttls => {
            let mut client = Client::new(tcp);
            read_greeting(&mut client).await?;
            client
                .run_command_and_check_ok("STARTTLS", None)
                .await
                .context("IMAP STARTTLS was refused")?;
            let tcp = client.into_inner();
            let tls = tls_connect(host, tcp).await?;
            Client::new(Box::new(tls) as BoxStream)
        }
        MailSecurity::Plain => {
            let mut client = Client::new(Box::new(tcp) as BoxStream);
            read_greeting(&mut client).await?;
            client
        }
    };
    client
        .login(&config.username, secret)
        .await
        .map_err(|(error, _)| anyhow::anyhow!("IMAP login failed: {error}"))
}

/// Metadata-only inbox polling for human-declared external waits. No body fetch or flag change.
#[derive(Debug)]
pub struct InboxCursor {
    pub uid_validity: u32,
    pub next_uid: u32,
    pub unread_uid: Option<u32>,
}

pub async fn poll_inbox(
    home: &std::path::Path,
    config: &EmailConfig,
    baseline: Option<(u32, u32)>,
) -> Result<InboxCursor> {
    let secret = config
        .secret(home)
        .context("email credential is unavailable")?;
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut session = imap_session(config, &secret).await?;
        let folder = session
            .examine("INBOX")
            .await
            .context("IMAP EXAMINE failed")?;
        let uid_validity = folder
            .uid_validity
            .context("IMAP server did not supply UIDVALIDITY")?;
        let next_uid = folder
            .uid_next
            .filter(|v| *v > 0)
            .context("IMAP server did not supply UIDNEXT")?;
        let unread_uid = if let Some((validity, first_uid)) = baseline {
            anyhow::ensure!(
                validity == uid_validity && next_uid >= first_uid,
                "mailbox generation changed; review waiting task"
            );
            session
                .uid_search(format!("UNSEEN UID {first_uid}:*"))
                .await
                .context("IMAP UID SEARCH failed")?
                .into_iter()
                .filter(|uid| *uid >= first_uid)
                .min()
        } else {
            None
        };
        let _ = session.logout().await;
        Ok(InboxCursor {
            uid_validity,
            next_uid,
            unread_uid,
        })
    })
    .await
    .context("IMAP inbox poll timed out")?
}

async fn read_greeting<T: AsyncRead + AsyncWrite + Unpin + Debug>(
    client: &mut Client<T>,
) -> Result<()> {
    client
        .read_response()
        .await?
        .context("IMAP server closed before its greeting")?;
    Ok(())
}

async fn tls_connect(
    host: &str,
    tcp: tokio::net::TcpStream,
) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .context("IMAP TLS protocol configuration failed")?
    .with_root_certificates(roots)
    .with_no_client_auth();
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .context("invalid IMAP TLS server name")?;
    tokio_rustls::TlsConnector::from(Arc::new(config))
        .connect(server_name, tcp)
        .await
        .context("IMAP TLS handshake failed")
}

#[cfg(test)]
#[path = "email_tests.rs"]
mod tests;
