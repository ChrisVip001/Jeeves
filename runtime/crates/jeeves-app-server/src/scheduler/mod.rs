//! Scheduled jobs, fired by the resident Runtime (`attach_scheduler`).
//!
//! Two declaration sources, one shared semantics:
//!
//! * **Skills**: add a line `schedule: 0 8 * * 1-5` to the SKILL.md frontmatter.
//!   A skill already says "how to do things"; adding a time makes it "do this
//!   every weekday at 8am" — scheduled jobs need no separate format.
//! * **The `schedule_jobs` table** (state.db): one-off jobs that don't fit a skill, written by the
//!   `schedule_job` tool or the capability center. An old `<home>/schedules.json` is imported once.
//!
//! On trigger it runs **one turn in the same session** (`schedule:<name>`), so each
//! execution carries the previous context and rolling summary; results go into
//! `<home>/outbox/`, not into chat — scheduled output is a file to read at leisure.
//!
//! At most one trigger per minute (`last_fired` records to the minute); jobs run
//! serially: same-minute tasks run one by one, never hammering the model at once.
//! The Runtime owns the clock: any process that runs `app-server` fires due jobs; `claim_once` makes
//! two processes safe (the second finds the slot already claimed).
//!
//! cron is a **hand-written five-field matcher** (min hour day month weekday),
//! supporting `*`, `*/step`, `a-b` ranges, `a,b` lists — no dependency for one field.
//!
//! State and crash recovery (M4b: dispatch advances next_run first): schedule
//! state lives in state.db's `schedule_state` table. Before firing,
//! `claim` the slot, then execute; after settling, `confirm` — a crash in between
//! requires human reconciliation. `catch_up` applies only to missed triggers that were never claimed.
//!
//! Each job also has `delivery`: `outbox` (default, writes a file) | `notify` (file
//! plus notifier, for people away from the screen) | `quiet` (log only, so high-
//! frequency jobs don't spam the disk); empty replies (or `NO_REPLY`) skip delivery.
//!
//! Missed while down: at startup, look back over the minutes since the last fire
//! and **re-run only the latest one** (three days down still catches up once, not
//! a chain; lookback capped at seven days). Missed fires are not silently swallowed.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use crate::Server;
use anyhow::Result;
use chrono::{Local, NaiveDateTime, Timelike, Utc};

mod fire;
pub mod gap;
pub(crate) mod morning;
use fire::fire;

/// Where output is delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Writes `<home>/outbox/` (default — scheduled output is a file read at leisure).
    Outbox,
    /// Writes the file **and** pushes a notifier (Slack/ntfy) — off-screen too.
    Notify,
    /// Logs only: high-frequency jobs (watching CI every 15 min) shouldn't spray files.
    Quiet,
}

impl Delivery {
    fn parse(text: &str) -> Delivery {
        match text.trim().to_ascii_lowercase().as_str() {
            "notify" => Delivery::Notify,
            "quiet" => Delivery::Quiet,
            "outbox" => Delivery::Outbox,
            other => {
                eprintln!(
                    "(jeeves) unknown delivery '{other}' (outbox/notify/quiet); treating it as outbox"
                );
                Delivery::Outbox
            }
        }
    }
}

/// One scheduled job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: String,
    /// Five-field cron: `min hour day month weekday`.
    pub cron: String,
    /// The prompt sent to Jeeves when it fires.
    pub prompt: String,
    /// Catch up missed, unclaimed triggers. Interrupted claims require human review.
    pub catch_up: bool,
    /// How output is delivered. Defaults to Outbox.
    pub delivery: Delivery,
}

/// Collects jobs from skills and the `schedule_jobs` table. A table that cannot be read is
/// reported and treated as empty for this tick: skill jobs must still fire.
pub async fn load_jobs(home: &Path, pool: &sqlx::SqlitePool) -> Vec<Job> {
    let mut jobs = Vec::new();

    for skill in jeeves_memory::skills::loaded_skills(home) {
        if let Some(cron) = skill.schedule {
            jobs.push(Job {
                name: skill.name.clone(),
                cron,
                prompt: format!(
                    "Run skill '{name}' once the way it says: {description}",
                    name = skill.name,
                    description = skill.description
                ),
                catch_up: true,
                delivery: Delivery::Outbox,
            });
        }
    }

    match jeeves_state::ScheduleJobs::new(pool.clone()).list().await {
        Ok(rows) => jobs.extend(rows.into_iter().map(|job| Job {
            name: job.name,
            cron: job.cron,
            prompt: job.prompt,
            catch_up: job.catch_up,
            delivery: Delivery::parse(&job.delivery),
        })),
        Err(error) => eprintln!("(jeeves) the scheduled-job table could not be read: {error}"),
    }

    jobs
}

/// Five-field cron match — **implemented in `jeeves-app-server`** and re-exported here.
///
/// The capability center shows "next run" for the same patterns this process fires on, and
/// two matchers would drift into "the panel says 08:00, nothing ran at 08:00". One copy, two
/// callers; the tests below test the shared one.
pub use jeeves_agent::cron::matches as cron_matches;

/// Which jobs should fire this minute. `fired` maps "job → minute last fired";
/// the same job is handed out only once per minute.
pub fn due_now<'a>(
    jobs: &'a [Job],
    at: NaiveDateTime,
    fired: &HashMap<String, String>,
) -> Vec<&'a Job> {
    let minute_key = at.format("%Y-%m-%dT%H:%M").to_string();
    jobs.iter()
        .filter(|job| cron_matches(&job.cron, at))
        .filter(|job| fired.get(&job.name) != Some(&minute_key))
        .collect()
}

/// One ledger line per run (JSONL). The outbox is output (quiet has no output
/// at all); this ledger answers
/// "what ran and how each ended" — inspection, debugging, and frequency math.
fn run_log_line(
    name: &str,
    minute: &str,
    status: &str,
    detail: &str,
    reply_chars: usize,
    delivered: bool,
) -> String {
    let record = serde_json::json!({
        "ts": Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "name": name,
        "minute": minute,
        "status": status,
        "detail": detail,
        "reply_chars": reply_chars,
        "delivered": delivered,
    });
    record.to_string()
}

/// Appends one line to `<home>/schedule-runs.jsonl`. Side path: a write failure
/// only reports to stderr — a broken ledger must not turn a success into an error.
fn append_run_log(home: &Path, line: &str) {
    use std::io::Write as _;
    let path = home.join("schedule-runs.jsonl");
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{line}") {
                eprintln!(
                    "(jeeves) could not write the run ledger in {}: {e}",
                    path.display()
                );
            }
        }
        Err(e) => eprintln!(
            "(jeeves) could not open the run ledger {}: {e}",
            path.display()
        ),
    }
}

/// Whether a trigger was missed from `from` to `to` (inclusive, per-minute scan).
/// Returns the **last** missed point — down three days, catch up once, not a chain.
pub fn last_missed(job: &Job, from: NaiveDateTime, to: NaiveDateTime) -> Option<NaiveDateTime> {
    let mut latest = None;
    let mut t = from;
    let step = chrono::TimeDelta::try_minutes(1).expect("once a minute is always valid");
    while t <= to {
        if cron_matches(&job.cron, t) {
            latest = Some(t);
        }
        t += step;
    }
    latest
}

/// Resident loop: every 20 seconds, check whether anything should fire. Skill
/// changes are rescanned each tick — effectively hot reload.
pub async fn run(server: Server) -> Result<()> {
    let home = server.settings().home.clone();
    let schedules = jeeves_state::Schedules::new(server.state_pool());
    let mut fired: HashMap<String, String> = HashMap::new();

    match jeeves_tools::schedules::import_legacy_file(&home, &server.state_pool()).await {
        Ok(0) => {}
        Ok(count) => eprintln!(
            "(jeeves) imported {count} job(s) from schedules.json into the state database; the file is now schedules.json.migrated"
        ),
        Err(why) => eprintln!("(jeeves) {why}"),
    }
    let pool = server.state_pool();
    let briefing =
        server.settings().morning_briefing.as_deref().and_then(
            |raw| match morning::Briefing::parse(raw) {
                Ok(briefing) => Some(briefing),
                Err(error) => {
                    eprintln!("(jeeves) morning briefing disabled: {error}");
                    None
                }
            },
        );
    let initial = runtime_jobs(&home, &pool, briefing).await;
    if initial.is_empty() {
        eprintln!(
            "(jeeves) no scheduled jobs. Add a `schedule: 0 8 * * 1-5` line to a SKILL.md frontmatter, \
             or ask Jeeves to schedule one."
        );
    } else {
        for job in &initial {
            eprintln!("(jeeves) scheduled job {} — cron '{}'", job.name, job.cron);
        }
    }

    crate::wake::reconcile_unsettled_schedules(&server).await;
    let startup = Utc::now();
    for job in initial {
        let zone = briefing
            .filter(|_| job.name == morning::NAME)
            .map(|briefing| briefing.zone);
        let now0 = morning::wall(startup, zone);
        let delivery = match job.delivery {
            Delivery::Outbox => "outbox",
            Delivery::Notify => "notify",
            Delivery::Quiet => "quiet",
        };
        if let Err(e) = schedules.ensure(&job.name, job.catch_up, delivery).await {
            eprintln!(
                "(jeeves) could not record the state of job '{}' (continuing as if there were no history): {e}",
                job.name
            );
            continue;
        }

        match schedules.pending_claim(&job.name).await {
            Ok(Some(claimed)) => {
                eprintln!("(jeeves) job '{}' has an unsettled trigger at {claimed}; review external effects, then use jeeves schedule acknowledge <name>. It is not replayed.",job.name);
                fired.insert(job.name.clone(), now0.format("%Y-%m-%dT%H:%M").to_string());
                continue;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!(
                    "(jeeves) schedule claim could not be checked; skipping '{}': {error}",
                    job.name
                );
                continue;
            }
        }

        if let Some(last) = schedules
            .last_fired(&job.name)
            .await
            .ok()
            .flatten()
            .and_then(|text| NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M").ok())
        {
            fired.insert(job.name.clone(), last.format("%Y-%m-%dT%H:%M").to_string());
            let from = (last
                + chrono::TimeDelta::try_minutes(1).expect("once a minute is always valid"))
            .max(now0 - chrono::TimeDelta::try_days(7).expect("once a week is always valid"));
            if let Some(missed) = job
                .catch_up
                .then(|| last_missed(&job, from, now0))
                .flatten()
            {
                if zone.is_some() && missed.date() != now0.date() {
                    eprintln!(
                        "(jeeves) no catch-up for a morning briefing from a prior local date"
                    );
                    continue;
                }
                eprintln!(
                    "(jeeves) job '{}' missed its {} trigger; catching up now",
                    job.name,
                    missed.format("%Y-%m-%d %H:%M")
                );
                let catch_up = Job {
                    prompt: format!(
                        "{}\n\n(catch-up: the {} trigger was missed — the process was not running then.)",
                        job.prompt,
                        missed.format("%Y-%m-%d %H:%M")
                    ),
                    ..job.clone()
                };
                let minute = now0.format("%Y-%m-%dT%H:%M").to_string();
                if let Err(e) = fire(&server, &catch_up, now0).await {
                    eprintln!(
                        "(jeeves) the catch-up run of job '{}' failed: {e}",
                        job.name
                    );
                }
                fired.insert(job.name.clone(), minute);
            }
        }
    }

    let mut last_tick = startup;
    loop {
        server.flush_completion_notices().await;
        let now = Utc::now();
        if let Err(error) = crate::responsibility::tick(&server, now).await {
            eprintln!("(jeeves) responsibility clock: {error}");
        }
        if let Err(error) =
            crate::interaction::price_watch::tick(&server, now.timestamp_millis()).await
        {
            eprintln!("(jeeves) price watch clock: {error}");
        }
        let jobs = runtime_jobs(&home, &pool, briefing).await;
        let (briefs, ordinary): (Vec<_>, Vec<_>) = jobs
            .into_iter()
            .partition(|job| briefing.is_some() && job.name == morning::NAME);
        tick(&server, &ordinary, last_tick, now, None, &mut fired).await;
        if let Some(briefing) = briefing {
            tick(
                &server,
                &briefs,
                last_tick,
                now,
                Some(briefing.zone),
                &mut fired,
            )
            .await;
        }
        last_tick = now;
        tokio::time::sleep(Duration::from_secs(20)).await;
    }
}

/// The built-in duty is opt-in via boot settings, never activated by a model-authored job.
async fn runtime_jobs(
    home: &Path,
    pool: &sqlx::SqlitePool,
    briefing: Option<morning::Briefing>,
) -> Vec<Job> {
    let mut jobs = load_jobs(home, pool).await;
    if let Some(briefing) = briefing {
        jobs.retain(|job| job.name != morning::NAME);
        jobs.push(briefing.job());
    }
    jobs
}

async fn tick(
    server: &Server,
    jobs: &[Job],
    last_tick: chrono::DateTime<Utc>,
    now: chrono::DateTime<Utc>,
    zone: Option<chrono_tz::Tz>,
    fired: &mut HashMap<String, String>,
) {
    let (before, at) = (morning::wall(last_tick, zone), morning::wall(now, zone));
    if zone.is_some() {
        let bucket = |time: NaiveDateTime| {
            time.date()
                .and_hms_opt(time.hour(), time.minute(), 0)
                .expect("valid wall-clock minute")
        };
        let (begin, end) = (bucket(before), bucket(at));
        if gap::is_gap(begin, end) {
            gap::reconcile(server, jobs, begin, end, fired).await;
        }
    } else if gap::is_gap(before, at) {
        gap::reconcile(server, jobs, before, at, fired).await;
    }
    for job in due_now(jobs, at, fired) {
        fired.insert(job.name.clone(), at.format("%Y-%m-%dT%H:%M").to_string());
        if let Err(error) = fire(server, job, at).await {
            eprintln!("(jeeves) job '{}' failed: {error}", job.name);
        }
    }
}

/// Human review releases an interrupted slot without rerunning its work.
pub async fn acknowledge(server: &Server, name: &str) -> Result<()> {
    let rows = jeeves_state::schedule_completions::ScheduleCompletions::new(server.state_pool());
    match rows.acknowledge(name).await? {
        Some(minute) => {
            append_run_log(
                &server.settings().home,
                &run_log_line(
                    name,
                    &minute,
                    "reviewed",
                    "Human acknowledged the prior effects; no replay",
                    0,
                    false,
                ),
            );
            println!("Acknowledged '{name}' at {minute}; the old trigger will not be replayed.");
        }
        None => println!("No unsettled trigger for '{name}'."),
    }
    Ok(())
}

#[cfg(test)]
mod tests;

/// Starts the clock inside this Runtime process. Fire-and-forget: the loop ends with the process.
pub fn attach_scheduler(server: Server) {
    tokio::spawn(async move {
        if let Err(error) = run(server).await {
            eprintln!("(jeeves) the scheduler stopped: {error}");
        }
    });
}
