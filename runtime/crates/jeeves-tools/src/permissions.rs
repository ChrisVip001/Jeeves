//! The host owns authorization; tools know only this boundary.
use crate::{
    approval::{ApprovalBroker, ApprovalFut, ApprovalRequest},
    ToolCtx,
};
use serde_json::Value;
use std::{future::Future, pin::Pin};

pub struct Authorization {
    pub revision: i32,
    pub approved: bool,
}
pub type AuthorizationFuture = Pin<Box<dyn Future<Output = Result<Authorization, String>> + Send>>;
pub trait ExecutionPolicy: Send + Sync {
    fn authorize(
        &self,
        ctx: ToolCtx,
        name: String,
        effect: crate::ToolEffect,
        args: Value,
    ) -> AuthorizationFuture;
    fn validate(&self, session_id: &str, revision: i32) -> Result<(), String>;
}

/// Scoped to the already approved handler invocation, not retained for the next tool.
pub struct ApprovedInvocation;
impl ApprovalBroker for ApprovedInvocation {
    fn request(&self, _: ApprovalRequest) -> ApprovalFut {
        Box::pin(async { true })
    }
}

/// An allowlist, not tool-provided metadata. Unknown/MCP tools are not proven reads.
pub fn proven_read(name: &str) -> bool {
    matches!(
        name,
        "browser_observe"
            | "read_file"
            | "glob"
            | "grep"
            | "find_symbol"
            | "repo_map"
            | "search_web"
            | "fetch_url"
            | "read_page"
            | "list_schedules"
            | "find_tools"
    )
}

/// Like [`proven_read`], but looks at the arguments too: the merged `memory` tool reads for
/// `search` / `list` and writes for everything else. Only the authorization gate has the
/// arguments; name-only callers keep the conservative answer (`memory` is not a read).
pub fn proven_read_call(name: &str, args: &serde_json::Value) -> bool {
    if name == crate::memory_tool::MEMORY {
        return args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::memory_tool::is_read_action);
    }
    if name == "job" {
        return args
            .get("action")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|action| matches!(action, "output" | "list"));
    }
    proven_read(name)
}

impl crate::ToolRegistry {
    /// Ordinary chat with an unresolved effect accepts only unconditional reads.
    /// Main chat can also use the read actions of mixed tools under its execution policy.
    pub fn restrict_to_reads(&mut self, include_read_actions: bool) {
        let mut mixed = Vec::new();
        if include_read_actions {
            for (name, description, fields) in [
                (
                    crate::memory_tool::MEMORY,
                    "Search or list stored memories.",
                    &["action", "query", "top_k", "limit"][..],
                ),
                (
                    "job",
                    "List background jobs or read their output.",
                    &["action", "id", "cursor", "wait"][..],
                ),
            ] {
                let Some(mut tool) = self.get(name).cloned() else {
                    continue;
                };
                let properties = tool.input_schema["properties"]
                    .as_object_mut()
                    .expect("built-in tool properties");
                properties.retain(|key, _| fields.contains(&key.as_str()));
                properties["action"]["enum"]
                    .as_array_mut()
                    .expect("built-in action enum")
                    .retain(|action| {
                        proven_read_call(name, &serde_json::json!({"action": action}))
                    });
                tool.description = description.into();
                mixed.push(tool);
            }
        }
        self.tools.retain(|name, _| proven_read(name));
        self.validators
            .retain(|name, _| self.tools.contains_key(name));
        self.effects.retain(|name, _| self.tools.contains_key(name));
        self.exclusive.retain(|name| self.tools.contains_key(name));
        self.deferred.retain(|name| self.tools.contains_key(name));
        for tool in mixed {
            self.register_exclusive_with_effect(tool, crate::ToolEffect::Read);
        }
    }
}

pub fn redact(mut value: serde_json::Value) -> serde_json::Value {
    match &mut value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_lowercase();
                if [
                    "key",
                    "token",
                    "secret",
                    "password",
                    "authorization",
                    "cookie",
                ]
                .iter()
                .any(|part| key.contains(part))
                {
                    *value = serde_json::Value::String("[redacted]".into());
                } else {
                    *value = redact(value.take());
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                *item = redact(item.take());
            }
        }
        _ => {}
    }
    value
}
