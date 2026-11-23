//! Optional outbound connector adapters. The caller supplies the final execution fence.
use crate::{EventSink, Server};
use anyhow::{Context, Result, ensure};
use jeeves_protocol::ResponsibilityDelivery;
use std::{future::Future, path::Path, pin::Pin};

type Check<'a> =
    Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> + Send + 'a>;
type Publish<'a> = Pin<Box<dyn Future<Output = Result<(&'static str, String)>> + Send + 'a>>;

pub(crate) struct Outbound {
    pub id: &'static str,
    pub label: &'static str,
    ready: fn(&Path) -> Result<bool>,
    send: for<'a> fn(Request<'a>, Check<'a>) -> Publish<'a>,
}
pub(crate) struct Request<'a> {
    pub server: &'a Server,
    pub sink: &'a EventSink,
    pub key: &'a str,
    pub recipient: &'a str,
    pub body: &'a str,
}
impl Outbound {
    pub(crate) fn ready(&self, home: &Path) -> Result<bool> {
        (self.ready)(home)
    }

    pub(crate) async fn send<'a>(
        &self,
        request: Request<'a>,
        check: Check<'a>,
    ) -> Result<(&'static str, String)> {
        (self.send)(request, check).await
    }
}
mod feishu;

const CONNECTORS: &[Outbound] = &[feishu::OUTBOUND];

pub(crate) fn resolve(
    delivery: &ResponsibilityDelivery,
) -> Result<Option<(&'static Outbound, &str)>> {
    let (id, recipient) = match delivery {
        ResponsibilityDelivery::Connector {
            connector,
            recipient,
        } => (connector.as_str(), recipient.as_str()),
        ResponsibilityDelivery::Feishu { recipient } => ("feishu", recipient.as_str()),
        _ => return Ok(None),
    };
    ensure!(
        !recipient.trim().is_empty() && recipient.len() <= 256,
        "Select a valid recipient"
    );
    let connector = CONNECTORS
        .iter()
        .find(|adapter| adapter.id == id)
        .context("This outbound capability is unavailable")?;
    Ok(Some((connector, recipient)))
}

pub(crate) fn enabled(home: &Path) -> Vec<serde_json::Value> {
    CONNECTORS.iter().filter(|adapter|(adapter.ready)(home).unwrap_or(false)).map(|adapter|
        serde_json::json!({"kind":"connector","connector":adapter.id,"label":adapter.label,"reviewRequired":true})
    ).collect()
}
