//! Dispatch a verified draft through the connector's existing scoped approval and read-back.
use crate::{EventSink, Server};
use anyhow::{ensure, Context, Result};
use chrono::Utc;
use jeeves_connector::{
    config,
    feishu::{self, Delivery, TokenCache},
};
use jeeves_protocol::BudgetView;
use jeeves_state::responsibility_runs as store;
use std::time::Instant;

pub(super) async fn deliver(
    server: &Server,
    sink: &EventSink,
    key: &str,
    recipient: &str,
    body: &str,
    budget: &BudgetView,
    deadline: Instant,
) -> Result<(&'static str, String)> {
    store::proposal(&server.pool, key, body).await?;
    let home = server.settings().home;
    let Some(conf) = config::load(&home)?.feishu else {
        return Ok((
            "draft",
            "Verified proposal saved; Feishu is not configured".into(),
        ));
    };
    ensure!(
        conf.permits(recipient),
        "The weekly recipient is outside the Feishu allowlist"
    );
    if !conf.live {
        return Ok((
            "draft",
            "Verified proposal saved; Feishu is in draft mode".into(),
        ));
    }
    ensure!(
        budget.tools.is_none_or(|limit| limit >= 2),
        "Tool budget covers research but not a Feishu send"
    );
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
            ensure!(
                Instant::now() < deadline,
                "Weekly report wall budget exhausted"
            );
            ensure!(
                store::admit_send(&server.pool, key, Utc::now().timestamp_millis()).await?,
                "This responsibility or wake is no longer active"
            );
            Ok(())
        },
    )
    .await?;
    Ok(match outcome {
        Delivery::Denied => ("denied", "No send approval was granted".into()),
        Delivery::Sent {
            confirmed: true,
            receipt,
            ..
        } => ("confirmed", receipt),
        Delivery::Sent { receipt, .. } => ("review", receipt),
    })
}
