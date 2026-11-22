//! Scheduled jobs as tools: the model can create, list and cancel one from a sentence.
//!
//! Jobs live in state.db (`schedule_jobs`), next to the runtime state of each firing. The same
//! functions below serve three doors: these tools, the `schedule/*` methods behind the capability
//! center, and the scheduler itself, so they cannot disagree about what a valid job is.
//!
//! The Runtime owns the clock: any process that runs `app-server` fires due jobs. A job saved here
//! fires only while some Runtime is up, and the answers say so.

use std::path::Path;
use std::sync::Arc;

use jeeves_agent::schedule::{self, JobBody};
use jeeves_state::{ScheduleJobRow, ScheduleJobs};
use serde_json::{json, Value};
use sqlx::SqlitePool;

use crate::{require_str, Tool, ToolCtx};

/// The next fire, or the sentence saying why there is none.
fn next_label(cron: &str) -> String {
    let now = chrono::Local::now().naive_local();
    match schedule::next_run_at(cron, now) {
        Ok(when) => format!("Next run {when}"),
        Err(why) => format!("({why}, so it will not run)"),
    }
}

/// Skill-declared job names: a stored job may not shadow one.
pub fn reserved(home: &Path) -> Vec<String> {
    jeeves_memory::skills::loaded_skills(home)
        .into_iter()
        .filter(|skill| skill.schedule.is_some())
        .map(|skill| skill.name)
        .collect()
}

/// Validates and stores one job; the answer is a sentence for the panel or the model.
pub async fn save_job(home: &Path, pool: &SqlitePool, body: &JobBody) -> Result<String, String> {
    let job = schedule::validate(body, &reserved(home))?;
    ScheduleJobs::new(pool.clone())
        .save(&ScheduleJobRow {
            name: job.name.clone(),
            cron: job.cron.clone(),
            prompt: job.prompt,
            delivery: job.delivery.clone(),
            catch_up: job.catch_up,
        })
        .await
        .map_err(|error| format!("could not save the job: {error}"))?;
    Ok(format!(
        "saved \"{}\": {}, delivery {}",
        job.name, job.cron, job.delivery
    ))
}

/// Deletes one stored job; a skill-owned name is refused with the way out.
pub async fn remove_job(home: &Path, pool: &SqlitePool, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Which one should be deleted? The name is empty".to_string());
    }
    schedule::refuse_skill_owned(name, &reserved(home))?;
    match ScheduleJobs::new(pool.clone()).remove(name).await {
        Ok(true) => Ok(format!("deleted \"{name}\"")),
        Ok(false) => Err(format!("there is no stored job \"{name}\"")),
        Err(error) => Err(format!("could not delete the job: {error}")),
    }
}

/// One-time import of the old `<home>/schedules.json` into the table; the file is renamed to
/// `schedules.json.migrated` so it is never read again. Entries that already exist in the table
/// win (the import must not undo a later edit). Returns how many jobs came over.
pub async fn import_legacy_file(home: &Path, pool: &SqlitePool) -> Result<usize, String> {
    let path = home.join("schedules.json");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        format!(
            "{} is not valid JSON ({error}); it was left in place, fix it by hand and restart",
            path.display()
        )
    })?;
    let jobs = ScheduleJobs::new(pool.clone());
    let mut imported = 0;
    for entry in value
        .get("jobs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let text_of = |key: &str| entry.get(key).and_then(Value::as_str);
        let (Some(name), Some(cron), Some(prompt)) =
            (text_of("name"), text_of("cron"), text_of("prompt"))
        else {
            continue;
        };
        let body = JobBody {
            name: name.to_string(),
            cron: cron.to_string(),
            prompt: prompt.to_string(),
            delivery: text_of("delivery").unwrap_or("outbox").to_string(),
            catch_up: entry.get("catch_up").and_then(Value::as_bool),
        };
        let valid = match schedule::validate(&body, &[]) {
            Ok(valid) => valid,
            Err(why) => {
                eprintln!("(jeeves) schedules.json: skipped \"{name}\": {why}");
                continue;
            }
        };
        let row = ScheduleJobRow {
            name: valid.name,
            cron: valid.cron,
            prompt: valid.prompt,
            delivery: valid.delivery,
            catch_up: valid.catch_up,
        };
        if jobs
            .insert_if_absent(&row)
            .await
            .map_err(|error| format!("import failed: {error}"))?
        {
            imported += 1;
        }
    }
    let migrated = home.join("schedules.json.migrated");
    std::fs::rename(&path, &migrated)
        .map_err(|error| format!("imported, but could not rename the file: {error}"))?;
    Ok(imported)
}

fn body_from(args: &Value) -> JobBody {
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    JobBody {
        name: text("name"),
        cron: text("cron"),
        prompt: text("prompt"),
        delivery: {
            let delivery = text("delivery");
            if delivery.is_empty() {
                "outbox".to_string()
            } else {
                delivery
            }
        },
        catch_up: args.get("catch_up").and_then(Value::as_bool),
    }
}

pub fn schedule_tools() -> Vec<Tool> {
    vec![schedule_job(), list_schedules(), cancel_schedule()]
}

fn schedule_job() -> Tool {
    Tool {
        name: "schedule_job".to_string(),
        description: "Create a scheduled job: when (five-field cron) and what (one sentence). At that time it runs as a turn and the result goes to <home>/outbox/. The Runtime fires it while it is up. Use it when the user says things like \"send me a briefing at eight every day\"."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The job name: lowercase letters, digits, hyphens, e.g. morning-news"},
                "cron": {"type": "string", "description": "Five-field cron: minute hour day month weekday, e.g. `0 8 * * 1-5` for 08:00 on weekdays"},
                "prompt": {"type": "string", "description": "The sentence sent to Jeeves when it fires"},
                "delivery": {
                    "type": "string",
                    "enum": ["outbox", "notify", "quiet"],
                    "description": "outbox (default, writes a file) / notify (file + notification) / quiet (log only)"
                },
                "catch_up": {"type": "boolean", "description": "Catch up once after a missed run? Default true"}
            },
            "required": ["name", "cron", "prompt"]
        }),
        handler: Arc::new(|ctx: ToolCtx, args: Value| {
            Box::pin(async move {
                let body = body_from(&args);
                let summary = save_job(&ctx.home, &ctx.chat.pool(), &body)
                    .await
                    .map_err(|why| anyhow::anyhow!(why))?;
                let cron = body.cron.split_whitespace().collect::<Vec<_>>().join(" ");
                Ok(format!(
                    "{summary}.{}.\n\n                     It fires while the Runtime is up (the cockpit shows this job under Capabilities → Scheduled jobs).",
                    next_label(&cron)
                ))
            })
        }),
    }
}

fn list_schedules() -> Tool {
    Tool {
        name: "list_schedules".to_string(),
        description: "List the scheduled jobs (name, cron, prompt, delivery). Use it when the user asks what is scheduled. Last and next run times are in the cockpit."
            .to_string(),
        input_schema: json!({"type": "object", "properties": {}, "required": []}),
        handler: Arc::new(|ctx: ToolCtx, _args: Value| {
            Box::pin(async move {
                let (stored, error) = match ScheduleJobs::new(ctx.chat.pool()).list().await {
                    Ok(rows) => (rows, None),
                    Err(why) => (Vec::new(), Some(format!("the job table could not be read: {why}"))),
                };
                let mut lines: Vec<String> = Vec::new();
                for skill in jeeves_memory::skills::loaded_skills(&ctx.home) {
                    let Some(cron) = skill.schedule else { continue };
                    lines.push(format!(
                        "- {} (comes with a skill) · {cron} · runs that skill's method",
                        skill.name
                    ));
                }
                for job in stored {
                    lines.push(format!(
                        "- {} · {} · {}",
                        job.name,
                        job.cron,
                        job.prompt.chars().take(60).collect::<String>()
                    ));
                }
                if lines.is_empty() {
                    return Ok(
                        "No scheduled jobs yet. To add one, tell me when (cron) and what.".to_string()
                    );
                }
                let mut out = format!("{} scheduled jobs:\n{}", lines.len(), lines.join("\n"));
                if let Some(why) = error {
                    out.push_str(&format!("\nNote: {why}"));
                }
                out.push_str("\nJobs fire while the Runtime is up.");
                Ok(out)
            })
        }),
    }
}

fn cancel_schedule() -> Tool {
    Tool {
        name: "cancel_schedule".to_string(),
        description:
            "Delete a scheduled job (by name; only the stored ones). Use it when the user says \"cancel that job\"; a job that comes with a skill has to be changed in its SKILL.md, and this tool refuses and says so."
                .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The job name (check with list_schedules first)"}
            },
            "required": ["name"]
        }),
        handler: Arc::new(|ctx: ToolCtx, args: Value| {
            Box::pin(async move {
                let name = require_str(&args, "name")?;
                remove_job(&ctx.home, &ctx.chat.pool(), &name)
                    .await
                    .map_err(|why| anyhow::anyhow!(why))
            })
        }),
    }
}
