//! Background jobs: a long command shouldn't hold a whole conversation turn.
//!
//! A `cargo test` takes three minutes. Foreground `run_command` would **block the turn**:
//! a frozen UI, and nothing else entering the context. Background jobs split the two: the
//! command runs in its own process, output goes into a bounded ring the model reads on demand.
//!
//! Three rules:
//!
//! * **Authorization via the owner fence, not id secrecy**: a job id is a restart-safe random
//!   string, but only **the spawning session** can read or stop it; another session gets nothing.
//! * **Output is a bounded ring**: overflow drops the oldest bytes; reads **honestly report
//!   how much was lost** instead of erroring — a runaway command must not eat memory or fail reads.
//! * **Same gate as foreground**: background commands still pass vet and approval (exec.rs's
//!   `pass_gate`) — "background it and run at leisure" is the most natural way to bypass approval.
//!
//! Lifecycle: jobs live and die with the Jeeves process; children are reaped (`kill_on_drop`),
//! so "resume after restart" is meaningless here — not a daemon, just this run's helper.

use serde_json::{json, Value};

use crate::Tool;

/// A job's id (a restart-safe random string; see module docs).
pub type JobId = String;

/// A read of output.
#[derive(Debug, Clone)]
pub struct JobRead {
    /// New output starting from `cursor`.
    pub text: String,
    /// Where to resume next time.
    pub cursor: u64,
    pub running: bool,
    /// The ring overflowed: the bytes `dropped` mentions are gone.
    pub lossy: bool,
    pub dropped: u64,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct JobSummary {
    pub id: JobId,
    pub command: String,
    pub running: bool,
    pub exit_code: Option<i32>,
    pub bytes: u64,
}

/// The host capability for background jobs. The tool layer knows only this trait; the
/// implementation (process + ring + spill) lives in app-server — same discipline as `SubagentRunner`.
pub type JobFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;

pub trait JobRegistry: Send + Sync {
    /// Spawn a job. `owner` is the session name (the authorization fence); `cwd` means the
    /// same as foreground `run_command`'s `cwd` (writable roots are computed from it).
    fn spawn(
        &self,
        owner: &str,
        command: &str,
        cwd: Option<String>,
    ) -> JobFut<Result<JobId, String>>;
    /// Host-assigned task execution identity. Custom hosts may override this to persist it.
    fn spawn_for_run(
        &self,
        owner: &str,
        command: &str,
        cwd: Option<String>,
        _run_id: Option<String>,
    ) -> JobFut<Result<JobId, String>> {
        self.spawn(owner, command, cwd)
    }
    /// A checked host retains the permit beyond spawn, through process settlement.
    fn spawn_guarded(
        &self,
        owner: &str,
        command: &str,
        cwd: Option<String>,
        run_id: Option<String>,
        lease: Option<crate::resources::Lease>,
        worktree: Option<std::path::PathBuf>,
    ) -> JobFut<Result<JobId, String>> {
        if lease.is_some() || worktree.is_some() {
            return Box::pin(async {
                Err("This job host cannot retain mutation ownership".into())
            });
        }
        self.spawn_for_run(owner, command, cwd, run_id)
    }

    /// Read a slice. With `wait` true, wait a bounded while (until a result or the cap).
    fn output(
        &self,
        owner: &str,
        id: &JobId,
        cursor: u64,
        wait: bool,
    ) -> JobFut<Result<JobRead, String>>;
    /// Stop it. Returns "was it still running".
    fn kill(&self, owner: &str, id: &JobId) -> JobFut<Result<bool, String>>;
    /// This session's own jobs (nobody else's).
    fn list(&self, owner: &str) -> Vec<JobSummary>;
}

/// The answer to the model when no job host is present (background is off by default).
fn no_registry() -> String {
    "Error: background jobs are not enabled on this machine (they are switched on together with run_command: JEEVES_EXEC=1).".to_string()
}

/// `job_output`: read a slice of output. To continue reading, pass back the `cursor` you got.
pub fn job_output(jobs: Option<std::sync::Arc<dyn JobRegistry>>) -> Tool {
    Tool {
        name: "job_output".to_string(),
        description: "Read the output of a background job. Leave cursor off the first time (read from the start), then pass the cursor from the previous reply to carry on. While the job is still running, wait=true waits for a bounded while."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string", "description": "the job id (run_command with background=true returns one)" },
                "cursor": { "type": "integer", "minimum": 0, "description": "where to carry on from (given in the previous reply)" },
                "wait": { "type": "boolean", "description": "wait a while if it has not finished (bounded; past that it honestly reports it is still running)" }
            }
        }),
        handler: std::sync::Arc::new(move |ctx: crate::ToolCtx, args: Value| {
            let jobs = jobs.clone();
            Box::pin(async move {
                let Some(jobs) = jobs else {
                    return Ok(no_registry());
                };
                let Some(id) = args.get("id").and_then(Value::as_str) else {
                    return Ok("Error: missing id".to_string());
                };
                let cursor = args.get("cursor").and_then(Value::as_u64).unwrap_or(0);
                let wait = args.get("wait").and_then(Value::as_bool).unwrap_or(false);

                match jobs
                    .output(&ctx.session_id, &id.to_string(), cursor, wait)
                    .await
                {
                    Ok(read) => Ok(render(&read)),
                    Err(why) => Ok(format!("Error: {why}")),
                }
            })
        }),
    }
}

/// `job_list`: which jobs this session spawned.
pub fn job_list(jobs: Option<std::sync::Arc<dyn JobRegistry>>) -> Tool {
    Tool {
        name: "job_list".to_string(),
        description: "List the background jobs started in this session (only your own)."
            .to_string(),
        input_schema: json!({ "type": "object", "properties": {} }),
        handler: std::sync::Arc::new(move |ctx: crate::ToolCtx, _args: Value| {
            let jobs = jobs.clone();
            Box::pin(async move {
                let Some(jobs) = jobs else {
                    return Ok(no_registry());
                };
                let list = jobs.list(&ctx.session_id);
                if list.is_empty() {
                    return Ok("This session has no background jobs yet.".to_string());
                }
                let mut out = String::from("Background jobs:\n");
                for job in list {
                    out.push_str(&format!(
                        "- {} {} ({} bytes): {}\n",
                        job.id,
                        match job.exit_code {
                            Some(code) => format!("finished (exit code {code})"),
                            None => "still running".to_string(),
                        },
                        job.bytes,
                        job.command
                    ));
                }
                Ok(out.trim_end().to_string())
            })
        }),
    }
}

/// `job_kill`: stop it.
pub fn job_kill(jobs: Option<std::sync::Arc<dyn JobRegistry>>) -> Tool {
    Tool {
        name: "job_kill".to_string(),
        description: "Stop a background job (and its process group).".to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["id"],
            "properties": { "id": { "type": "string" } }
        }),
        handler: std::sync::Arc::new(move |ctx: crate::ToolCtx, args: Value| {
            let jobs = jobs.clone();
            Box::pin(async move {
                let Some(jobs) = jobs else {
                    return Ok(no_registry());
                };
                let Some(id) = args.get("id").and_then(Value::as_str) else {
                    return Ok("Error: missing id".to_string());
                };
                match jobs.kill(&ctx.session_id, &id.to_string()).await {
                    Ok(true) => Ok(format!("Stopped job {id}.")),
                    Ok(false) => Ok(format!("job {id} has already finished; nothing to stop.")),
                    Err(why) => Ok(format!("Error: {why}")),
                }
            })
        }),
    }
}

/// `job`: the one model-facing tool for background jobs. `action` is `output` (read a slice),
/// `list` (this session's jobs) or `kill`. Three schemas became one; the handlers are the same.
pub fn job(jobs: Option<std::sync::Arc<dyn JobRegistry>>) -> Tool {
    let output = job_output(jobs.clone());
    let list = job_list(jobs.clone());
    let kill = job_kill(jobs);
    Tool {
        name: "job".to_string(),
        description: "Work with background jobs started by run_command(background=true). action=output reads a job's output (leave cursor off the first time, then pass back the cursor from the previous reply; wait=true waits a bounded while for a running job). action=list shows this session's jobs. action=kill stops a job and its process group."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["output", "list", "kill"] },
                "id": { "type": "string", "description": "output / kill: the job id" },
                "cursor": { "type": "integer", "minimum": 0, "description": "output: where to carry on from" },
                "wait": { "type": "boolean", "description": "output: wait a while if it has not finished" }
            }
        }),
        handler: std::sync::Arc::new(move |ctx: crate::ToolCtx, args: Value| {
            let output = output.handler.clone();
            let list = list.handler.clone();
            let kill = kill.handler.clone();
            Box::pin(async move {
                match args.get("action").and_then(Value::as_str) {
                    Some("output") => output(ctx, args).await,
                    Some("list") => list(ctx, args).await,
                    Some("kill") => kill(ctx, args).await,
                    _ => Ok("Error: action must be output / list / kill.".to_string()),
                }
            })
        }),
    }
}

/// Render a slice of output into text for the model.
fn render(read: &JobRead) -> String {
    let mut out = String::new();
    if read.lossy {
        out.push_str(&format!(
            "(the first {} bytes were dropped: too much output, so the ring keeps only the most recent)\n",
            read.dropped
        ));
    }
    if read.running {
        out.push_str(&format!(
            "job is still running ({} read so far)",
            read.cursor
        ));
    } else {
        out.push_str(&format!(
            "job finished (exit code {})",
            read.exit_code.map(|c| c.to_string()).unwrap_or("?".into())
        ));
    }
    out.push('\n');
    if read.text.is_empty() {
        out.push_str("(no new output in this stretch)\n");
    } else {
        out.push_str(&read.text);
        if !read.text.ends_with('\n') {
            out.push('\n');
        }
    }
    if read.running {
        out.push_str(&format!("(to carry on, pass cursor={})", read.cursor));
    }
    out
}
