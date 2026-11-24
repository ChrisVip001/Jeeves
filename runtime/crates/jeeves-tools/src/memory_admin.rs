//! manage_memory — the maintenance surface for memory.
//!
//! Query (search_memory) and list (list_memory) are everyday actions; this tool handles
//! edit and delete: change a fact's body, or delete a fact/episode by id. Operations go
//! by **id** (from list_memory / search_memory output), not by content — deleting by
//! content would take out good memories on the same topic.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{Tool, ToolCtx, require_str};
use jeeves_memory::skills::is_slug;

const SKILL_APPROVAL_TIMEOUT_SECS: i64 = 120;

fn opt_i64(args: &Value, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| {
            anyhow::anyhow!("Parameter '{key}' should be an integer (the memory id), got {v}")
        }),
    }
}

pub fn manage_memory() -> Tool {
    Tool {
        name: "manage_memory".to_string(),
        description: "Maintain memory: change the text of a fact, or delete a fact/episode by id. The ids come from list_memory / search_memory output. Use it when the user says \"that one is wrong, fix it\" or \"delete that\"."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["update_fact", "delete_fact", "delete_episode"],
                    "description": "update_fact rewrites a fact; delete_fact deletes a fact; delete_episode deletes an episode"
                },
                "id": {"type": "integer", "description": "the memory id (from list/search output)"},
                "content": {"type": "string", "description": "the new text when action=update_fact"}
            },
            "required": ["action", "id"]
        }),
        handler: Arc::new(|ctx: ToolCtx, args: Value| {
            Box::pin(async move {
                let action = require_str(&args, "action")?;
                let id = opt_i64(&args, "id")?
                    .ok_or_else(|| anyhow::anyhow!("Missing parameter 'id' (the memory id)"))?;
                match action.as_str() {
                    "update_fact" => {
                        let content = require_str(&args, "content")?;
                        let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
                        let change = jeeves_memory::store::Change::Update { id, content: &content };
                        if jeeves_memory::store::apply(&ctx.facts, &bundle, change).await?
                            == jeeves_memory::store::Outcome::Updated(true)
                        {
                            Ok(format!("Fact {id} now reads: \"{content}\"."))
                        } else {
                            Ok(format!("There is no fact with id {id} in memory. Check the id with list_memory first."))
                        }
                    }
                    "delete_fact" => {
                        let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
                        if jeeves_memory::store::apply(&ctx.facts, &bundle, jeeves_memory::store::Change::Delete { id }).await?
                            == jeeves_memory::store::Outcome::Deleted(true)
                        {
                            Ok(format!("Deleted fact {id}."))
                        } else {
                            Ok(format!("There is no fact with id {id} in memory. Check the id with list_memory first."))
                        }
                    }
                    "delete_episode" => {
                        if ctx.episodes.delete(id).await? {
                            Ok(format!("Deleted episode {id}."))
                        } else {
                            Ok(format!("There is no episode with id {id}."))
                        }
                    }
                    other => Ok(format!(
                        "Unknown action '{other}'. Options: update_fact / delete_fact / delete_episode."
                    )),
                }
            })
        }),
    }
}

/// create_skill — turns a user-taught workflow into a SKILL.md (procedural memory's write entry).
///
/// The handler asks the human through `ApprovalBroker` before it writes: a model approving
/// its own code of conduct would be the same accident as an unguarded update_soul. Denial,
/// timeout and a turn with nobody to answer all leave the disk untouched.
/// Never overwrite an existing skill — built-in or user-written alike.
pub fn create_skill() -> Tool {
    Tool {
        name: "create_skill".to_string(),
        description: "Write a reusable workflow as a SKILL.md (procedural memory) that loads automatically the next time a related message appears. **Only call it after the user agrees.** body holds step-by-step instructions; description says when to use it (including trigger words)."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "a lowercase slug, e.g. weekly-review"},
                "description": {"type": "string", "description": "one sentence: what it does and when to use it (including trigger words)"},
                "body": {"type": "string", "description": "step-by-step instructions (markdown)"}
            },
            "required": ["name", "description", "body"]
        }),
        handler: Arc::new(|ctx: ToolCtx, args: Value| {
            Box::pin(async move {
                let name = require_str(&args, "name")?.to_lowercase().replace(' ', "-");
                let description = require_str(&args, "description")?;
                let body = require_str(&args, "body")?;

                if !is_slug(&name) {
                    return Ok(
                        "The skill name must be a short slug, e.g. 'weekly-review' (lowercase letters, digits, hyphens)."
                            .to_string(),
                    );
                }
                let dir = ctx.home.join("skills").join(&name);
                let dest = dir.join("SKILL.md");
                if dest.exists() {
                    return Ok(format!("A skill named '{name}' already exists — pick another name."));
                }
                let text =
                    format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n");
                if jeeves_memory::skills::parse_skill_text(&text).is_none() {
                    return Ok("This content failed validation — both name and description are required.".to_string());
                }
                let Some(broker) = ctx.approval.clone() else {
                    return Ok("Writing a skill needs a person's approval, but nobody can answer here — the skill was not created.".to_string());
                };
                let outcome = broker
                    .request(crate::approval::ApprovalRequest {
                        tool: "memory/create_skill".to_string(),
                        args_preview: format!(
                            "Create '{name}' at {}:\n\n{text}",
                            dest.display()
                        ),
                        reason: format!(
                            "Teach Jeeves a reusable workflow at {} — only you can agree to it",
                            dest.display()
                        ),
                        timeout_secs: SKILL_APPROVAL_TIMEOUT_SECS,
                        remember: None,
                        action: None,
                    })
                    .await;
                if !outcome.approved() {
                    return Ok(outcome.refusal("skill write"));
                }
                if dest.exists() {
                    return Ok(format!(
                        "A skill named '{name}' was created while you were deciding — nothing was overwritten."
                    ));
                }
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&dest, text)?;

                Ok(format!(
                    "Skill '{name}' created ({}). It takes effect automatically when a message like \"{description}\" comes up.",
                    dest.display()
                ))
            })
        }),
    }
}
