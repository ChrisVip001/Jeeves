//! A clock that jumped forward (the machine slept, or the process was frozen) is not the same as a
//! process that was down. The resident loop notices the jump and runs one reconcile pass over the
//! gap, with the same rules as startup catch-up: a trigger that was never claimed and is inside
//! `catch_up` fires once (the latest one, not a chain); a claimed one is never replayed; a job with
//! `catch_up` off is reported as missed and left alone. Nothing here is silent.

use chrono::{NaiveDateTime, TimeDelta};

use super::{Job, last_missed};

/// The loop ticks every 20 s; more than this between two ticks means the clock jumped.
pub const GAP_THRESHOLD_SECS: i64 = 90;
/// Same cap as startup catch-up: older misses are not worth a scan.
const LOOKBACK_DAYS: i64 = 7;

/// Did the wall clock advance by more than a tick could explain? A clock moving backwards is not a gap.
pub fn is_gap(last_tick: NaiveDateTime, now: NaiveDateTime) -> bool {
    (now - last_tick).num_seconds() > GAP_THRESHOLD_SECS
}

/// What a job missed inside the gap: the latest trigger strictly after `after` (the last confirmed
/// fire, or the last tick) and strictly before the current minute, which the normal path owns.
pub fn missed_in_gap(job: &Job, after: NaiveDateTime, now: NaiveDateTime) -> Option<NaiveDateTime> {
    let minute = TimeDelta::try_minutes(1).expect("one minute is valid");
    let from = (after + minute).max(now - TimeDelta::try_days(LOOKBACK_DAYS).expect("days"));
    let to = now - minute;
    if from > to {
        return None;
    }
    last_missed(job, from, to)
}

/// What to do with one job's miss.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Nothing,
    Fire(NaiveDateTime),
    /// `catch_up` is off: say so, do not run.
    Report(NaiveDateTime),
    /// A claimed trigger is unsettled: never replayed, a human decides.
    Blocked,
}

pub fn verdict(job: &Job, missed: Option<NaiveDateTime>, claim_pending: bool) -> Verdict {
    match (missed, claim_pending) {
        (None, _) => Verdict::Nothing,
        (Some(_), true) => Verdict::Blocked,
        (Some(at), false) if job.catch_up => Verdict::Fire(at),
        (Some(at), false) => Verdict::Report(at),
    }
}

/// The reconcile pass over a gap. Every outcome is a stderr line; a fired catch-up carries a note
/// in its prompt saying why it ran late.
pub(super) async fn reconcile(
    server: &crate::Server,
    jobs: &[Job],
    last_tick: NaiveDateTime,
    now: NaiveDateTime,
    fired: &mut std::collections::HashMap<String, String>,
) {
    let schedules = jeeves_state::Schedules::new(server.state_pool());
    eprintln!(
        "(jeeves) the clock jumped from {} to {} (the machine slept?); checking for missed triggers",
        last_tick.format("%Y-%m-%d %H:%M"),
        now.format("%Y-%m-%d %H:%M")
    );
    for job in jobs {
        let after = match schedules.last_fired(&job.name).await {
            Ok(Some(text)) => NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M")
                .ok()
                .map_or(last_tick, |fired_at| fired_at.max(last_tick)),
            _ => last_tick,
        };
        let missed = missed_in_gap(job, after, now);
        let pending = matches!(schedules.pending_claim(&job.name).await, Ok(Some(_)));
        match verdict(job, missed, pending) {
            Verdict::Nothing => {}
            Verdict::Blocked => eprintln!(
                "(jeeves) job '{}' missed a trigger while the clock jumped, but it has an unsettled one; not replayed. Review, then `jeeves schedule acknowledge {}`",
                job.name, job.name
            ),
            Verdict::Report(at) => eprintln!(
                "(jeeves) job '{}' missed its {} trigger while the clock jumped; catch_up is off, so it was not run",
                job.name,
                at.format("%Y-%m-%d %H:%M")
            ),
            Verdict::Fire(at) => {
                eprintln!(
                    "(jeeves) job '{}' missed its {} trigger while the clock jumped; running it once now",
                    job.name,
                    at.format("%Y-%m-%d %H:%M")
                );
                let late = Job {
                    prompt: format!(
                        "{}\n\n(catch-up: the {} trigger was missed because the machine was asleep.)",
                        job.prompt,
                        at.format("%Y-%m-%d %H:%M")
                    ),
                    ..job.clone()
                };
                if let Err(error) = super::fire::fire(server, &late, now).await {
                    eprintln!(
                        "(jeeves) the catch-up run of job '{}' failed: {error}",
                        job.name
                    );
                }
                fired.insert(job.name.clone(), now.format("%Y-%m-%dT%H:%M").to_string());
            }
        }
    }
}
