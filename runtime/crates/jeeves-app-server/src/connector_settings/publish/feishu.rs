//! Feishu integration with account, destination and read-back verification.
use super::{Check, Outbound, Request};
use anyhow::{Context, Result, ensure};
use jeeves_connector::{
    config,
    feishu::{self, Delivery, TokenCache},
};

pub(super) const OUTBOUND: Outbound = Outbound {
    id: "feishu",
    label: "Feishu",
    ready: |home| Ok(config::load(home)?.feishu.is_some_and(|c| c.live)),
    send: |request, check| Box::pin(publish(request, check)),
};

async fn publish(request: Request<'_>, check: Check<'_>) -> Result<(&'static str, String)> {
    let Request {
        server,
        sink,
        key,
        recipient,
        body,
    } = request;
    let home = server.settings().home;
    let Some(conf) = config::load(&home)?.feishu else {
        return Ok(("draft", "Draft saved; Feishu is not configured".into()));
    };
    ensure!(
        conf.permits(recipient),
        "The recipient is outside the Feishu allowlist"
    );
    if !conf.live {
        return Ok(("draft", "Draft saved; Feishu is in draft mode".into()));
    }
    let approval = server.approval_bridge(key, sink);
    let ctx = server.tool_ctx(key, None, approval);
    let outcome = feishu::send_with_receipt(
        &ctx,
        &conf,
        &TokenCache::default(),
        recipient,
        body,
        Some(key),
        || async {
            let latest = config::load(&home)?
                .feishu
                .context("Feishu was disconnected")?;
            ensure!(
                latest.live
                    && latest.permits(recipient)
                    && latest.app_id == conf.app_id
                    && latest.base_url == conf.base_url
                    && latest.identity() == conf.identity()
                    && latest.receive_id_type == conf.receive_id_type,
                "Feishu scope changed after proposal"
            );
            check().await
        },
    )
    .await?;
    Ok(match outcome {
        Delivery::Denied(outcome) => ("denied", outcome.refusal("send")),
        Delivery::Sent {
            confirmed: true,
            receipt,
            ..
        } => ("confirmed", receipt),
        Delivery::Sent { receipt, .. } => ("review", receipt),
    })
}
