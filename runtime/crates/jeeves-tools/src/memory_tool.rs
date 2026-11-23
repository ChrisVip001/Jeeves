//! The model-facing memory surface: two tools, `memory` and `read_page`.
//!
//! Before the merge the model saw eight small tools (`save_note`, `forget_note`,
//! `search_memory`, `list_memory`, `manage_memory`, `create_skill`, `read_memory_page`,
//! `read_skill`). Every schema rides in every request, so eight became two: the action is
//! a parameter, the handlers underneath are unchanged and keep their own tests.
//!
//! Permission note: `search` and `list` are reads, the rest write. The gate is told through
//! [`is_read_action`], so a read-only session can still look things up.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::{Tool, ToolCtx, require_str};

/// Tool name of the merged memory tool.
pub const MEMORY: &str = "memory";
/// Tool name of the merged page reader.
pub const READ_PAGE: &str = "read_page";

/// Actions of `memory` that only read.
pub fn is_read_action(action: &str) -> bool {
    matches!(action, "search" | "list")
}

/// Map a `memory` action to the handler that implements it, and the `action` value that
/// handler expects (only `manage_memory` takes one).
fn route(action: &str) -> Option<(&'static str, Option<&'static str>)> {
    Some(match action {
        "save" => ("save_note", None),
        "forget" => ("forget_note", None),
        "search" => ("search_memory", None),
        "list" => ("list_memory", None),
        "update" => ("manage_memory", Some("update_fact")),
        "delete" => ("manage_memory", Some("delete_fact")),
        "delete_episode" => ("manage_memory", Some("delete_episode")),
        "create_skill" => ("create_skill", None),
        _ => return None,
    })
}

const ACTIONS: &str =
    "save / forget / search / list / update / delete / delete_episode / create_skill";

/// `memory`: save, forget, search, list, correct and delete memories; write a skill.
pub fn memory() -> Tool {
    let mut inner: HashMap<String, Tool> = crate::handlers::memory_tools()
        .into_iter()
        .chain([
            crate::memory_admin::manage_memory(),
            crate::memory_admin::create_skill(),
        ])
        .map(|tool| (tool.name.clone(), tool))
        .collect();
    let inner = Arc::new(std::mem::take(&mut inner));
    Tool {
        name: MEMORY.to_string(),
        description: format!(
            "Jeeves's long-term memory. `action` picks the operation: {ACTIONS}. \
             save: remember a durable fact (subject + content). forget: drop everything about a subject — only when the user says so. \
             search: look memories up (query). list: the most recent facts (limit). \
             update / delete: fix or remove one fact by id (ids come from search/list); delete_episode removes an episode by id. \
             create_skill: write a reusable workflow as a SKILL.md (name, description, body) — only after the user agrees."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["save", "forget", "search", "list", "update", "delete", "delete_episode", "create_skill"]},
                "subject": {"type": "string", "description": "save / forget: who or what the fact is about"},
                "content": {"type": "string", "description": "save: the fact in one sentence; update: the corrected text"},
                "kind": {"type": "string", "description": "save: optional category (default fact)"},
                "query": {"type": "string", "description": "search: the search terms"},
                "top_k": {"type": "integer", "description": "search: how many at most (4 by default)"},
                "limit": {"type": "integer", "description": "list: how many (10 by default)"},
                "id": {"type": "integer", "description": "update / delete / delete_episode: the memory id"},
                "name": {"type": "string", "description": "create_skill: lowercase slug, e.g. weekly-review"},
                "description": {"type": "string", "description": "create_skill: what it does and when to use it"},
                "body": {"type": "string", "description": "create_skill: step-by-step instructions (markdown)"}
            },
            "required": ["action"]
        }),
        handler: Arc::new(move |ctx: ToolCtx, args: Value| {
            let inner = Arc::clone(&inner);
            Box::pin(async move {
                let action = require_str(&args, "action")?;
                let Some((tool, rewrite)) = route(&action) else {
                    return Ok(format!("Unknown action '{action}'. Options: {ACTIONS}."));
                };
                let mut forwarded = args.clone();
                if let (Some(rewrite), Some(object)) = (rewrite, forwarded.as_object_mut()) {
                    object.insert("action".into(), Value::String(rewrite.into()));
                }
                let handler = &inner[tool].handler;
                handler(ctx, forwarded).await
            })
        }),
    }
}

/// `read_page`: read one memory-library page or one skill body, by `kind`.
pub fn read_page() -> Tool {
    let mut inner: HashMap<String, Tool> = crate::handlers::memory_tools()
        .into_iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect();
    let inner = Arc::new(std::mem::take(&mut inner));
    Tool {
        name: READ_PAGE.to_string(),
        description: "Read the full text behind a catalog line in the system prompt. kind=memory reads a memory-library page (name = the page name, without .md) with its sources; kind=skill reads a skill's full steps (name = the skill name)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "kind": {"type": "string", "enum": ["memory", "skill"]},
                "name": {"type": "string", "description": "The page or skill name, as written in the catalog"}
            },
            "required": ["kind", "name"]
        }),
        handler: Arc::new(move |ctx: ToolCtx, args: Value| {
            let inner = Arc::clone(&inner);
            Box::pin(async move {
                let kind = require_str(&args, "kind")?;
                let name = require_str(&args, "name")?;
                match kind.as_str() {
                    "memory" => {
                        let handler = &inner["read_memory_page"].handler;
                        handler(ctx, json!({ "page": name })).await
                    }
                    "skill" => {
                        let handler = &inner["read_skill"].handler;
                        handler(ctx, json!({ "name": name })).await
                    }
                    other => Ok(format!("Unknown kind '{other}'. Options: memory / skill.")),
                }
            })
        }),
    }
}
