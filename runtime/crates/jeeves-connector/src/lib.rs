//! Connectors — the agent acting **in the world**, one human nod at a time.
//!
//! The `send_message` tool produces a draft. Connectors deliver through the following controls:
//!
//!   1. **Off until configured.** No `<home>/connectors.json` → no connector tools exist;
//!      the model cannot even see their names.
//!   2. **Dry run by default.** A configured connector still writes drafts to
//!      `<home>/outbox/` unless the config says `"live": true`.
//!   3. **Recipient allowlist is a hard boundary.** Empty list = nobody. The model
//!      cannot widen it; only the human edits the file.
//!   4. **Every live action asks.** The approval card names the recipients and a body
//!      preview — a card that doesn't say what will happen is a card nobody can judge.

mod caldav_discovery;
pub mod calendar;
pub mod config;
pub mod email;
pub mod feishu;
pub mod feishu_oauth;

pub use config::{ConnectorConfig, FeishuConfig};
pub use feishu::feishu_send_tool;

#[cfg(test)]
#[path = "feishu_tests.rs"]
mod feishu_tests;

#[cfg(test)]
#[path = "feishu_oauth_tests.rs"]
mod feishu_oauth_tests;
