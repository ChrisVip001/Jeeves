//! One-shot jobs: a stored job with `run_at` fires once at that minute and is then removed.
//!
//! Its cron pins the same minute (`M H D Mo *`) for display, but the cron paths (startup catch-up,
//! the gap reconcile, `due_now`) skip it: that pattern also matches a year later. This module is
//! the only clock a one-shot job has. A missed one runs late once when `catch_up` is on and the
//! miss is within a week; otherwise it is reported, logged and removed. Each outcome is a stderr
//! line.

use chrono::{NaiveDateTime, TimeDelta, Timelike};

use super::{Job, append_run_log, run_log_line};

/// Same cap as the cron catch-up: a reminder more than a week late is noise, not help.
const LATE_LIMIT_DAYS: i64 = 7;

/// What a one-shot job needs this tick.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Wait,
    Fire,
    /// The minute passed while nothing was watching; run it now with a note.
    Late,
    /// Missed and not worth running (catch-up off, or too late): report and remove.
    Drop,
}

/// `attempted` = this process already handed this job out for this `run_at`; a slot claimed
/// elsewhere or held for review is not retried every 20 seconds.
pub fn step(run_at: NaiveDateTime, catch_up: bool, at: NaiveDateTime, attempted: bool) -> Step {
    let minute = at
        .with_second(0)
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(at);
    if attempted || run_at > minute {
        Step::Wait
    } else if run_at == minute {
        Step::Fire
    } else if catch_up && minute - run_at <= TimeDelta::days(LATE_LIMIT_DAYS) {
        Step::Late
    } else {
        Step::Drop
    }
}

/// The stored form `remove_one_shot` matches on.
pub fn key(run_at: NaiveDateTime) -> String {
    run_at
        .format(jeeves_agent::schedule::RUN_AT_FORMAT)
        .to_string()
}

/// Runs every one-shot job that is due or overdue at `at`.
pub(super) async fn tick(
    server: &crate::Server,
    jobs: &[Job],
    at: NaiveDateTime,
    fired: &mut std::collections::HashMap<String, String>,
) {
    for job in jobs {
        let Some(run_at) = job.run_at else { continue };
        let when = key(run_at);
        let marker = format!("once:{when}");
        let step = step(
            run_at,
            job.catch_up,
            at,
            fired.get(&job.name) == Some(&marker),
        );
        if step == Step::Wait {
            continue;
        }
        fired.insert(job.name.clone(), marker);
        let run = match step {
            Step::Late => {
                eprintln!(
                    "(jeeves) one-shot job '{}' was due at {when} while the Runtime was not running; running it once now",
                    job.name
                );
                Job {
                    prompt: format!(
                        "{}\n\n(late: this was due at {when}; the Runtime was not running then.)",
                        job.prompt
                    ),
                    ..job.clone()
                }
            }
            Step::Drop => {
                drop_missed(server, job, &when).await;
                continue;
            }
            _ => job.clone(),
        };
        if let Err(error) = super::fire::fire(server, &run, at).await {
            eprintln!("(jeeves) one-shot job '{}' failed: {error}", job.name);
        }
    }
}

async fn drop_missed(server: &crate::Server, job: &Job, when: &str) {
    let why = if job.catch_up {
        "more than a week late"
    } else {
        "catch_up is off"
    };
    eprintln!(
        "(jeeves) one-shot job '{}' missed its {when} time ({why}); not run, removed",
        job.name
    );
    remove(server, &job.name, when).await;
    append_run_log(
        &server.settings().home,
        &run_log_line(&job.name, when, "missed", why, 0, false),
    );
}

/// Removes the one-shot declaration; a failure is said, and the next start reports it again.
pub(super) async fn remove(server: &crate::Server, name: &str, when: &str) {
    match jeeves_state::ScheduleJobs::new(server.state_pool())
        .remove_one_shot(name, when)
        .await
    {
        Ok(true) => eprintln!("(jeeves) one-shot job '{name}' ({when}) removed"),
        Ok(false) => eprintln!(
            "(jeeves) one-shot job '{name}' was changed after {when} was set; the new declaration stays"
        ),
        Err(error) => {
            eprintln!("(jeeves) one-shot job '{name}' could not be removed: {error}")
        }
    }
}
