//! Built-in tools.
//!
//! Every tool's output **states truthfully where the result landed** (local file, state.db table).
//! Not politeness: the model relays it to the user; clear text avoids a false "synced to the cloud".

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{Tool, ToolCtx, opt_u32, require_str};

/// The four memory tools: save, forget, search, list.
/// They are the part of Jeeves that "manages its own memory" — when the user says
/// "forget this one", it goes through forget_note, not some hidden admin command.
pub fn memory_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "save_note".to_string(),
            description: "Store a fact worth remembering for the long run (about a person, a project, a preference). Right for things like \"Alex prefers morning meetings\"."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "Who or what the fact is about, e.g. alex or the acme project"},
                    "content": {"type": "string", "description": "The fact itself, in one sentence"}
                },
                "required": ["subject", "content"]
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let subject = require_str(&args, "subject")?;
                    let content = require_str(&args, "content")?;
                    let kind = args.get("kind").and_then(Value::as_str).unwrap_or("fact");
                    let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
                    let change = jeeves_memory::store::Change::Remember { subject: &subject, content: &content, source: "user", kind };
                    let jeeves_memory::store::Outcome::Remembered { row, is_new } =
                        jeeves_memory::store::apply(&ctx.facts, &bundle, change).await?
                    else {
                        unreachable!("Remember answers Remembered")
                    };
                    if !is_new {
                        return Ok(format!(
                            "Already recorded (not stored twice): #{} {} — {}",
                            row.id, row.subject, row.content
                        ));
                    }
                    Ok(format!(
                        "Remembered: **{}** — {} (in the facts table of {}, retrievable with memory(action=search) at any time)",
                        row.subject,
                        row.content,
                        ctx.home.join("state.db").display()
                    ))
                })
            }),
        },
        Tool {
            name: "forget_note".to_string(),
            description: "Forget every memory about a subject. Use it only when the user explicitly says to forget something.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "subject": {"type": "string", "description": "The subject to forget"}
                },
                "required": ["subject"]
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let subject = require_str(&args, "subject")?;
                    let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
                    let jeeves_memory::store::Outcome::Forgot(removed) = jeeves_memory::store::apply(
                        &ctx.facts,
                        &bundle,
                        jeeves_memory::store::Change::ForgetSubject { subject: &subject },
                    )
                    .await?
                    else {
                        unreachable!("ForgetSubject answers Forgot")
                    };
                    if removed == 0 {
                        Ok(format!("There is no memory about \"{subject}\", so nothing was deleted."))
                    } else {
                        Ok(format!("Forgot {removed} memories about \"{subject}\"."))
                    }
                })
            }),
        },
        Tool {
            name: "search_memory".to_string(),
            description: "Search the memories. Use it when the user asks about the past, a person or a preference.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The search terms"},
                    "top_k": {"type": "integer", "description": "How many at most, 4 by default"}
                },
                "required": ["query"]
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let query = require_str(&args, "query")?;
                    let top_k = opt_u32(&args, "top_k", 4)?;
                    let hits = ctx.facts.search(&query, top_k).await?;
                    if hits.is_empty() {
                        return Ok(format!("There is nothing in memory about \"{query}\"."));
                    }
                    let lines: Vec<String> = hits
                        .iter()
                        .map(|f| format!("- **{}**: {} (source {})", f.subject, f.content, f.source))
                        .collect();
                    Ok(format!("Found {}:\n{}", hits.len(), lines.join("\n")))
                })
            }),
        },
        Tool {
            name: "read_memory_page".to_string(),
            description: "Read one page of the memory library (the `[title](page.md)` name from the catalog in the system prompt). The catalog carries one-line summaries only; read the page for detail, sources or the exact wording."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "page": {"type": "string", "description": "The page name, e.g. alex (without .md)"}
                },
                "required": ["page"]
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let page = require_str(&args, "page")?;
                    let bundle = jeeves_memory::bundle::Bundle::new(&ctx.home);
                    match bundle.read(&page)? {
                        Some(found) => {
                            if !jeeves_memory::mirror::page_is_current(&ctx.facts, &found).await? {
                                return Ok("This memory page has obsolete or time-bounded sources and is excluded from recall. Search current memories instead.".to_string());
                            }
                            let sources = if found.sources.is_empty() {
                                "(this page records no sources)".to_string()
                            } else {
                                found.sources.join("、")
                            };
                            Ok(format!(
                                "# {} ({})\n\n{}\n\nSources: {}",
                                found.title, found.kind, found.body, sources
                            ))
                        }
                        None => Ok(format!(
                            "The memory library has no page \"{page}\". The readable pages are in the catalog in the system prompt."
                        )),
                    }
                })
            }),
        },
        Tool {
            name: "read_skill".to_string(),
            description: "Read a skill's full text (the skill catalog in the system prompt gives the names). The catalog carries one line each; the body is where the steps, commands and caveats live."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "The skill name — the one in `- name — description` in the catalog"}
                },
                "required": ["name"]
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let name = require_str(&args, "name")?;
                    let mut loader = jeeves_memory::skills::SkillLoader::new(
                        jeeves_memory::skills::SkillLoader::dirs_for(&ctx.home),
                    );
                    match loader.body_of(&name) {
                        Some(body) => Ok(format!("### {name}\n{body}")),
                        None => {
                            let known: Vec<String> = loader
                                .index_text(usize::MAX)
                                .lines()
                                .filter_map(|line| line.strip_prefix("- "))
                                .filter_map(|rest| rest.split(" — ").next())
                                .map(str::to_string)
                                .collect();
                            Ok(format!(
                                "No skill named \"{name}\". Known skills: {}",
                                if known.is_empty() {
                                    "(none)".to_string()
                                } else {
                                    known.join(", ")
                                }
                            ))
                        }
                    }
                })
            }),
        },
        Tool {
            name: "list_memory".to_string(),
            description: "List the most recently remembered facts. Use it when the user wants to know what Jeeves has remembered.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "description": "How many, 10 by default"}
                },
                "required": []
            }),
            handler: Arc::new(|ctx: ToolCtx, args: Value| {
                Box::pin(async move {
                    let limit = opt_u32(&args, "limit", 10)?;
                    let rows = ctx.facts.recent(limit, 0).await?;
                    if rows.is_empty() {
                        return Ok(
                            "Memory is still empty. Tell Jeeves something about yourself and it will remember.".to_string()
                        );
                    }
                    let lines: Vec<String> = rows
                        .iter()
                        .map(|f| format!("- **{}**: {}", f.subject, f.content))
                        .collect();
                    Ok(format!(
                        "The {} most recent memories:\n{}",
                        rows.len(),
                        lines.join("\n")
                    ))
                })
            }),
        },
    ]
}

/// Widens the session's tool table (`JEEVES_TOOL_POLICY=deferred`).
///
/// The model sees `find_tools` plus the tools every turn needs; everything else is one
/// search away. It is a **token** knob: nothing becomes permitted here, and a switch-gated
/// tool never hides (`ALWAYS_ON`), so discovery cannot outrun consent.
pub fn find_tools() -> Tool {
    Tool {
        name: "find_tools".to_string(),
        description: "When a capability is missing from the tool table, use this to find it by keyword and open it (usable next turn). E.g. \"calendar\", \"email\", \"spreadsheet\", \"screenshot\"."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "What capability to look for, e.g. \"calendar event\""}
            },
            "required": ["query"]
        }),
        handler: Arc::new(|ctx: ToolCtx, args: Value| {
            Box::pin(async move {
                let query = require_str(&args, "query")?;
                let Some(activate) = ctx.activate else {
                    return Ok(
                        "The tool table is already complete; there are no hidden tools — just use what you have.".to_string()
                    );
                };
                let opened = activate(&query);
                if opened.is_empty() {
                    return Ok(format!(
                        "No additional tool found for \"{query}\". Either the name is off, or that capability is not installed here."
                    ));
                }
                Ok(format!(
                    "Opened {} tools, usable next turn: {}",
                    opened.len(),
                    opened.join("、")
                ))
            })
        }),
    }
}

/// The full P2 built-in tool set. More get added behind flags later — the registry is assembled
/// **explicitly**: no macros, no directory scanning, so "why is this tool here" is checkable at a glance.
pub fn build_default() -> crate::ToolRegistry {
    build_default_with_web(None)
}

pub fn build_default_with_web(primary: Option<crate::Handler>) -> crate::ToolRegistry {
    let mut registry = crate::ToolRegistry::new();
    registry.register(crate::memory_tool::memory());
    registry.register(crate::memory_tool::read_page());
    registry.register(find_tools());
    for tool in crate::schedules::schedule_tools() {
        registry.register(tool);
    }
    registry.register(crate::messages::send_message());
    if primary.is_some() || crate::web::search_ready() {
        registry.register(crate::web::search_web_with_primary(primary.clone()));
    }
    registry.register(crate::web::fetch_url_with_primary(primary));
    registry.register(crate::fs::read_file());
    registry.register(crate::search::glob_tool());
    registry.register(crate::search::grep_tool());
    registry.register_with_effect(crate::search::find_symbol_tool(), crate::ToolEffect::Read);
    registry.register_with_effect(crate::search::repo_map_tool(), crate::ToolEffect::Read);
    registry
}
