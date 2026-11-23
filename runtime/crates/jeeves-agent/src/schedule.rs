//! What a scheduled job is, and when it is valid. Pure functions: no files, no database.
//!
//! The scheduler, the capability center and the `schedule_job` tool all accept a job from a
//! different door. Each refusal is a sentence that names the thing to fix; it goes straight to the
//! panel or back to the model, so it has to be actionable rather than a status code.

use chrono::{Datelike, NaiveDateTime, TimeDelta, Timelike};
use serde::Deserialize;

use crate::cron;

/// The delivery modes the scheduler understands. Anything else is refused rather than silently
/// treated as `outbox`: a job whose result goes somewhere the human did not expect is worse than a
/// job that refuses to be saved.
pub const DELIVERIES: [&str; 3] = ["outbox", "notify", "quiet"];

/// The fields a caller may set.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct JobBody {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cron: String,
    #[serde(default)]
    pub prompt: String,
    /// Empty = `outbox`.
    #[serde(default)]
    pub delivery: String,
    /// `None` = true: a missed report is worth re-running once.
    #[serde(default)]
    pub catch_up: Option<bool>,
    /// A local minute (`YYYY-MM-DD HH:MM`) for a job that runs once instead of on a cron.
    #[serde(default)]
    pub run_at: Option<String>,
}

/// A job that passed [`validate`]: trimmed, normalised, safe to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidJob {
    pub name: String,
    pub cron: String,
    pub prompt: String,
    pub delivery: String,
    pub catch_up: bool,
    /// Set for a one-shot job; `cron` then pins the same minute for display.
    pub run_at: Option<NaiveDateTime>,
}

/// The stored and displayed form of a one-shot time.
pub const RUN_AT_FORMAT: &str = "%Y-%m-%d %H:%M";

/// A one-shot job may be set at most this far ahead; past that its pinned cron would read the
/// same as a yearly job, and a typo in the year is the likelier story.
const RUN_AT_HORIZON_DAYS: i64 = 366;

/// Reads a one-shot time: `YYYY-MM-DD HH:MM`, a `T` separator, or with seconds (dropped).
pub fn parse_run_at(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    [
        "%Y-%m-%d %H:%M",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ]
    .iter()
    .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())
    .and_then(|at| at.with_second(0))
}

/// Checks a body. `reserved` lists names that already mean something else (skills that declare
/// their own `schedule:` line): the scheduler keys jobs by name, so two declarations sharing one
/// would fight, and refusing here says which one wins.
/// `now` is the local wall clock, used only to refuse a one-shot time that has already passed.
pub fn validate(
    body: &JobBody,
    reserved: &[String],
    now: NaiveDateTime,
) -> Result<ValidJob, String> {
    let name = body.name.trim();
    validate_name(name)?;
    if reserved.iter().any(|taken| taken == name) {
        return Err(format!(
            "{name} is already taken by a skill that carries a schedule — pick another name, or edit that skill's SKILL.md"
        ));
    }
    let run_at = match body.run_at.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) => Some(one_shot(text, &body.cron, now)?),
    };
    let cron = match run_at {
        Some(at) => format!(
            "{} {} {} {} *",
            at.minute(),
            at.hour(),
            at.day(),
            at.month()
        ),
        None => body.cron.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    if !cron::valid(&cron) {
        return Err(format!(
            "cron \"{}\" is invalid: five fields, e.g. `0 8 * * 1-5` (08:00 on weekdays)",
            body.cron.trim()
        ));
    }
    let prompt = body.prompt.trim();
    if prompt.is_empty() {
        return Err(
            "say what to do when it fires — this sentence is sent to Jeeves as a turn".to_string(),
        );
    }
    let delivery = if body.delivery.trim().is_empty() {
        "outbox".to_string()
    } else {
        body.delivery.trim().to_ascii_lowercase()
    };
    if !DELIVERIES.contains(&delivery.as_str()) {
        return Err(format!(
            "unknown delivery \"{delivery}\": outbox (write a file) / notify (file + notification) / quiet (log only)"
        ));
    }
    Ok(ValidJob {
        name: name.to_string(),
        cron,
        prompt: prompt.to_string(),
        delivery,
        catch_up: body.catch_up.unwrap_or(true),
        run_at,
    })
}

/// The checks only a one-shot time needs: it parses, it is not also a cron, it is in the future,
/// and it is within [`RUN_AT_HORIZON_DAYS`].
fn one_shot(text: &str, cron_text: &str, now: NaiveDateTime) -> Result<NaiveDateTime, String> {
    if !cron_text.trim().is_empty() {
        return Err("give either cron (repeats) or run_at (once), not both".to_string());
    }
    let Some(at) = parse_run_at(text) else {
        return Err(format!(
            "run_at \"{text}\" is not a time: use local YYYY-MM-DD HH:MM, e.g. 2026-11-25 15:00"
        ));
    };
    if at <= now {
        return Err(format!(
            "run_at {} has already passed (it is {} now)",
            at.format(RUN_AT_FORMAT),
            now.format(RUN_AT_FORMAT)
        ));
    }
    if at - now > TimeDelta::days(RUN_AT_HORIZON_DAYS) {
        return Err(format!(
            "run_at {} is more than a year away; check the year, or use a cron for a yearly job",
            at.format(RUN_AT_FORMAT)
        ));
    }
    Ok(at)
}

/// A refusal for deleting a name a skill owns.
pub fn refuse_skill_owned(name: &str, reserved: &[String]) -> Result<(), String> {
    if reserved.iter().any(|taken| taken == name) {
        return Err(format!(
            "{name} belongs to a skill — delete that skill, or remove the schedule line from its SKILL.md"
        ));
    }
    Ok(())
}

/// Lowercase letters, digits and hyphens — the name is the scheduler's key, and it travels into
/// `schedule:<name>` session ids.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err(
            "a job needs a name: lowercase letters, digits, hyphens, e.g. morning-news".to_string(),
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "the name \"{name}\" is invalid: only lowercase letters, digits and hyphens are allowed (it is the schedule key too)"
        ));
    }
    if name.len() > 64 {
        return Err("the name is too long (64 characters at most)".to_string());
    }
    Ok(())
}

/// The next fire as `YYYY-MM-DD HH:MM`, or the reason there is none. One formatter for the panel
/// and the model's answer: two would eventually disagree about what "next" means.
pub fn next_run_at(cron_text: &str, from: NaiveDateTime) -> Result<String, String> {
    cron::next_run(cron_text, from)
}

#[cfg(test)]
#[path = "schedule_tests.rs"]
mod tests;
