//! `feishu_send` — the second connector: a message into a Feishu/Lark chat or to a person.
//!
//! Three fences (allowlist → live switch → one approval per live send).
//! What differs is the transport: an app-identity `tenant_access_token`, fetched on
//! demand and cached **in memory only** — it expires in two hours and is derived from
//! the app secret, so writing another credential to disk would buy nothing.
//!
//! Identity boundary, stated plainly: messages go out **as the app**, which is what a
//! custom app can do. Acting as a specific user needs Feishu's OAuth code flow with a
//! console-registered redirect URI; that lands when there is a real app to verify it
//! against (see docs/limitations).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use jeeves_tools::approval::{ActionGrant, ApprovalRequest};
use jeeves_tools::{require_str, Tool, ToolCtx};
use serde_json::{json, Value};

use crate::config::FeishuConfig;

/// Approval wait.
const APPROVAL_TIMEOUT_SECS: i64 = 120;
/// A message API that takes 20s is down; don't hold the turn hostage.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// In-process token cache: (token, fetched_at). Public because the tool closure shares it
/// across turns — a token per turn would hammer the auth endpoint for no reason.
#[derive(Default)]
pub struct TokenCache {
    inner: Mutex<Option<(String, Instant)>>,
}

impl TokenCache {
    fn get(&self) -> Option<String> {
        let guard = self
            .inner
            .lock()
            .expect("the token lock should not be poisoned");
        guard
            .as_ref()
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(100 * 60))
            .map(|(token, _)| token.clone())
    }

    fn put(&self, token: String) {
        *self
            .inner
            .lock()
            .expect("the token lock should not be poisoned") = Some((token, Instant::now()));
    }
}

pub fn feishu_send_tool(config: FeishuConfig) -> Tool {
    let cache = Arc::new(TokenCache::default());
    Tool {
        name: "feishu_send".to_string(),
        description: "Send a text message to an allowlisted Feishu chat or member. By default it only writes a draft into the local outbox; actually sending needs live turned on in the config, and every send asks first."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "to": {"type": "string", "description": "recipient id (open_id or chat_id, must be allowlisted)"},
                "text": {"type": "string", "description": "message body (plain text)"},
                "dry_run": {"type": "boolean", "description": "keep a draft, do not send. Unset = follow the config"}
            },
            "required": ["to", "text"]
        }),
        handler: Arc::new(move |ctx: ToolCtx, args: Value| {
            let config = config.clone();
            let cache = cache.clone();
            Box::pin(async move { run(ctx, config, cache, args).await })
        }),
    }
}

async fn run(
    ctx: ToolCtx,
    config: FeishuConfig,
    cache: Arc<TokenCache>,
    args: Value,
) -> Result<String> {
    let to = require_str(&args, "to")?.trim().to_string();
    let text = require_str(&args, "text")?;
    let dry_run = args
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    if !config.permits(&to) {
        bail!(
            "Recipient {to} is not on the allowlist.\n\n             The allowlist is allow_recipients in {} (feishu section) — only a person can change it; empty means nobody is allowed.",
            ctx.home.join("connectors.json").display()
        );
    }

    if dry_run || !config.live {
        let path = write_draft(&ctx.home, &config, &to, &text).await?;
        return Ok(format!(
            "Feishu draft put in the outbox ({}): to {to}.{}",
            path.display(),
            if config.live {
                " this run's dry_run asked for it"
            } else {
                "; the connector is not live, so nothing was sent"
            }
        ));
    }

    match send_with_receipt(&ctx, &config, &cache, &to, &text, None, || async { Ok(()) }).await? {
        Delivery::Denied(outcome) => bail!("{}", outcome.refusal("send")),
        Delivery::Sent {
            message_id,
            receipt,
            ..
        } => Ok(format!(
            "Sent to {to} (Feishu message_id {message_id}). {receipt}"
        )),
    }
}

pub enum Delivery {
    /// The approval did not come back as a yes; nothing was sent.
    Denied(jeeves_tools::approval::ApprovalOutcome),
    Sent {
        message_id: String,
        confirmed: bool,
        receipt: String,
    },
}

/// Shared live boundary. A scheduled caller rechecks its lease after approval and token acquisition.
pub async fn send_with_receipt<F, Fut>(
    ctx: &ToolCtx,
    config: &FeishuConfig,
    cache: &TokenCache,
    to: &str,
    text: &str,
    receipt_owner: Option<&str>,
    before_post: F,
) -> Result<Delivery>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    anyhow::ensure!(
        config.live && config.permits(to),
        "live Feishu delivery to this recipient is disabled"
    );
    let Some(secret) = config.secret(&ctx.home) else {
        bail!(
            "the Feishu app_secret could not be found ({}) — put it in the keychain or an environment variable first.",
            config.app_secret
        );
    };

    let Some(broker) = ctx.approval.clone() else {
        bail!("Sending a Feishu message for real would have to ask a person first, and there is no interface to answer — nothing was sent this time.");
    };
    let content_hash = jeeves_state::ledger::digest_of(text.as_bytes());
    let identity = config.identity().map_err(anyhow::Error::msg)?;
    let account = format!("{}:{}:{identity}", config.base_url, config.app_id);
    let destination = format!("{}:{to}", config.receive_id_type);
    let request = ApprovalRequest {
        tool: "feishu_send".to_string(),
        args_preview: format!("Channel: Feishu\nAccount: {account}\nTo: {destination}\nContent digest: {content_hash}\n\n{text}"),
        reason: "the connector is live; every actual send is confirmed".to_string(),
        timeout_secs: APPROVAL_TIMEOUT_SECS,
        remember: None,
        action: Some(ActionGrant {
            account,
            resource: "feishu".into(),
            action: "send".into(),
            destination,
            content_hash: Some(content_hash),
            scope: "once".into(),
            expires_at: Some(Utc::now().timestamp() + APPROVAL_TIMEOUT_SECS),
        }),
    };
    let outcome = broker.request(request).await;
    if !outcome.approved() {
        return Ok(Delivery::Denied(outcome));
    }

    let token = match identity {
        "user" => crate::feishu_oauth::access_token(config, &ctx.home).await?,
        _ => match cache.get() {
            Some(token) => token,
            None => {
                let token = tenant_token(config, &secret).await?;
                cache.put(token.clone());
                token
            }
        },
    };
    before_post().await?;
    let message_id = send_message(config, &token, to, text).await?;
    let (confirmed, receipt) =
        record_receipt(ctx, config, &token, &message_id, receipt_owner).await;
    Ok(Delivery::Sent {
        message_id,
        confirmed,
        receipt,
    })
}

/// Claim a receipt for the send, then read the message back. Only the read-back can confirm it.
/// The message is already out, so a bookkeeping problem is reported in the text, never raised.
async fn record_receipt(
    ctx: &ToolCtx,
    config: &FeishuConfig,
    token: &str,
    message_id: &str,
    receipt_owner: Option<&str>,
) -> (bool, String) {
    let pool = ctx.chat.pool();
    let run_id = ctx.task_run_id.as_deref();
    let task_id = if let Some(owner) = receipt_owner {
        owner.to_owned()
    } else if let Some(run) = run_id {
        match jeeves_state::Runs::new(pool.clone()).get(run).await {
            Ok(Some(run)) => run.task_id,
            _ => return (false, "No receipt: the task run could not be found.".into()),
        }
    } else {
        return (false, "No receipt: this is not a task run.".into());
    };
    let receipts = jeeves_state::Receipts::new(pool);
    let reference = format!("feishu:{message_id}");
    let id = match receipts
        .claim(&task_id, run_id, "feishu_message", &reference)
        .await
    {
        Ok(id) => id,
        Err(error) => return (false, format!("No receipt: {error}")),
    };
    let seen = read_back(config, token, message_id).await;
    let settled = if seen {
        receipts.confirm(id, None).await
    } else {
        receipts.fail(id, None).await
    };
    let confirmed = settled.is_ok() && seen;
    let message = match (settled, seen) {
        (Err(error), _) => format!("Receipt {id} claimed but could not be settled: {error}"),
        (Ok(()), true) => format!("Receipt {id} confirmed by read-back."),
        (Ok(()), false) => {
            format!("Receipt {id} stays unconfirmed: the read-back did not see the message.")
        }
    };
    (confirmed, message)
}

/// `GET /im/v1/messages/{id}`: true only when Feishu answers `code 0` for that message.
async fn read_back(config: &FeishuConfig, token: &str, message_id: &str) -> bool {
    let url = format!(
        "{}/open-apis/im/v1/messages/{message_id}",
        config.base_url.trim_end_matches('/')
    );
    let Ok(client) = client() else { return false };
    let Ok(response) = client.get(url).bearer_auth(token).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(body) = response.json::<Value>().await else {
        return false;
    };
    body.get("code").and_then(Value::as_i64) == Some(0) && response_items_match(&body, message_id)
}

fn response_items_match(body: &Value, expected: &str) -> bool {
    body.pointer("/data/items")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .any(|item| item.get("message_id").and_then(Value::as_str) == Some(expected))
        })
}

/// The app-identity token. `code != 0` is a real answer from Feishu, quote it verbatim.
async fn tenant_token(config: &FeishuConfig, secret: &str) -> Result<String> {
    let response = client()?
        .post(format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            config.base_url.trim_end_matches('/')
        ))
        .json(&json!({ "app_id": config.app_id, "app_secret": secret }))
        .send()
        .await
        .context("could not reach the Feishu auth endpoint")?;
    let body: Value = response
        .json()
        .await
        .context("the Feishu auth endpoint did not return JSON")?;
    if body.get("code").and_then(Value::as_i64) != Some(0) {
        bail!(
            "Feishu refused to issue a token: {} {} (check app_id / app_secret and the app's availability scope)",
            body.get("code").unwrap_or(&Value::Null),
            body.get("msg").and_then(Value::as_str).unwrap_or("")
        );
    }
    body.get("tenant_access_token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the Feishu reply has no tenant_access_token"))
}

async fn send_message(config: &FeishuConfig, token: &str, to: &str, text: &str) -> Result<String> {
    let url = format!(
        "{}/open-apis/im/v1/messages?receive_id_type={}",
        config.base_url.trim_end_matches('/'),
        config.receive_id_type
    );
    let content = json!({ "text": text }).to_string();
    let response = client()?
        .post(url)
        .bearer_auth(token)
        .json(&json!({ "receive_id": to, "msg_type": "text", "content": content }))
        .send()
        .await
        .context("could not reach the Feishu message endpoint")?;
    let body: Value = response
        .json()
        .await
        .context("the Feishu message endpoint did not return JSON")?;
    if body.get("code").and_then(Value::as_i64) != Some(0) {
        bail!(
            "Feishu refused the send: {} {}",
            body.get("code").unwrap_or(&Value::Null),
            body.get("msg").and_then(Value::as_str).unwrap_or("")
        );
    }
    body.get("data")
        .and_then(|data| data.get("message_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .context("Feishu accepted the send without a message ID; review delivery")
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent(concat!("jeeves/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("could not build the HTTP client")
}

/// Send for real, with the token cache bypassed (CLI: one message, one process).
pub async fn send_live(
    config: &FeishuConfig,
    home: &std::path::Path,
    to: &str,
    text: &str,
) -> Result<String> {
    let Some(secret) = config.secret(home) else {
        bail!(
            "the Feishu app_secret could not be found ({}).",
            config.app_secret
        );
    };
    let token = match config.identity().unwrap_or("app") {
        "user" => crate::feishu_oauth::access_token(config, home).await?,
        _ => tenant_token(config, &secret).await?,
    };
    send_message(config, &token, to, text).await
}

pub async fn write_draft(
    home: &std::path::Path,
    config: &FeishuConfig,
    to: &str,
    text: &str,
) -> Result<std::path::PathBuf> {
    let dir = home.join("outbox");
    tokio::fs::create_dir_all(&dir).await?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S");
    let safe_to: String = to
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    let path = dir.join(format!("feishu-{stamp}-{safe_to}.txt"));
    let kind = if config.receive_id_type == "chat_id" {
        "chat_id"
    } else {
        "open_id"
    };
    let text = format!("To: {to} ({kind})\nApp: {}\n\n{text}\n", config.app_id);
    tokio::fs::write(&path, text).await?;
    Ok(path)
}
