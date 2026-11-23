//! Normalize owner-approved times and admit one durable occurrence per slot.
use super::Server;
use anyhow::{bail, ensure, Context, Result};
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use jeeves_protocol::{ResponsibilityPlan, ResponsibilityTrigger as Trigger};
use sqlx::SqlitePool;

const CATCH_UP_MS: i64 = 15 * 60_000;

fn instant(value: &str) -> Result<i64> {
    Ok(DateTime::parse_from_rfc3339(value)?.timestamp_millis())
}

fn utc(value: i64) -> Result<DateTime<Utc>> {
    DateTime::from_timestamp_millis(value).context("Time is out of range")
}

pub(super) fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
}

pub(super) fn validate(trigger: &Trigger, now: DateTime<Utc>, expiry: i64) -> Result<()> {
    match trigger {
        Trigger::After { delay_seconds } => {
            ensure!(*delay_seconds > 0, "Delay must be positive");
            ensure!(
                now.timestamp_millis() + i64::from(*delay_seconds) * 1000 < expiry,
                "The delay ends after the responsibility expires"
            );
        }
        Trigger::At { at } => ensure!(instant(at)? < expiry, "Choose a time before expiry"),
        Trigger::Every {
            interval_seconds,
            anchor,
        } => {
            ensure!(
                *interval_seconds >= 60,
                "Repeat intervals must be at least one minute"
            );
            if let Some(anchor) = anchor {
                ensure!(
                    instant(anchor)? < expiry,
                    "Choose an interval start before expiry"
                );
            }
        }
        Trigger::Cron {
            expression,
            time_zone,
        } => {
            ensure!(jeeves_agent::cron::valid(expression), "Invalid schedule");
            let zone: Tz = time_zone.parse()?;
            ensure!(
                next_cron(expression, zone, now)?.is_some(),
                "Schedule has no upcoming time"
            );
        }
        Trigger::Event { source, event_type } => {
            ensure!(
                slug(source) && slug(event_type),
                "Choose a configured event source and type"
            );
        }
    }
    Ok(())
}

pub(super) fn anchor(trigger: &mut Trigger, now: DateTime<Utc>) -> Result<()> {
    match trigger {
        Trigger::After { delay_seconds } => {
            ensure!(*delay_seconds > 0, "Delay must be positive");
            *trigger = Trigger::At {
                at: utc(now.timestamp_millis() + i64::from(*delay_seconds) * 1000)?.to_rfc3339(),
            };
        }
        Trigger::Every {
            anchor,
            interval_seconds,
        } if anchor.is_none() => {
            *anchor = Some(
                utc(now.timestamp_millis() + i64::from(*interval_seconds) * 1000)?.to_rfc3339(),
            );
        }
        _ => {}
    }
    Ok(())
}

fn next_cron(expression: &str, zone: Tz, after: DateTime<Utc>) -> Result<Option<i64>> {
    let minute = utc(after.timestamp_millis().div_euclid(60_000) * 60_000)?;
    let mut wall = minute.with_timezone(&zone).naive_local();
    for _ in 0..180 {
        let Some(candidate) = jeeves_agent::cron::next_after(expression, wall) else {
            return Ok(None);
        };
        wall = candidate;
        let at = match zone.from_local_datetime(&candidate) {
            LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => at.timestamp_millis(),
            LocalResult::None => continue,
        };
        if at > after.timestamp_millis() {
            return Ok(Some(at));
        }
    }
    bail!("Could not find the next local slot")
}

pub(super) fn first(trigger: &Trigger, now: DateTime<Utc>) -> Result<Option<i64>> {
    let ms = now.timestamp_millis();
    match trigger {
        Trigger::At { at } => Ok(Some(instant(at)?)),
        Trigger::Every {
            interval_seconds,
            anchor,
        } => {
            let step = i64::from(*interval_seconds) * 1000;
            ensure!(step > 0, "Interval must be positive");
            let phase = instant(
                anchor
                    .as_deref()
                    .context("Interval has no approved anchor")?,
            )?;
            Ok(Some(if phase >= ms {
                phase
            } else {
                phase + (ms - phase + step - 1).div_euclid(step) * step
            }))
        }
        Trigger::Cron {
            expression,
            time_zone,
        } => next_cron(expression, time_zone.parse()?, now),
        Trigger::Event { .. } => Ok(None),
        Trigger::After { .. } => bail!("Delay has not been approved"),
    }
}

fn slot(
    trigger: &Trigger,
    due: i64,
    now: DateTime<Utc>,
    window_ms: i64,
) -> Result<(i64, Option<i64>)> {
    let ms = now.timestamp_millis();
    let selected = match trigger {
        Trigger::Every {
            interval_seconds, ..
        } => {
            let step = i64::from(*interval_seconds) * 1000;
            ensure!(step > 0, "Interval must be positive");
            due + (ms - due).div_euclid(step) * step
        }
        Trigger::Cron {
            expression,
            time_zone,
        } => {
            let zone: Tz = time_zone.parse()?;
            let mut candidate = ms.div_euclid(60_000) * 60_000;
            let mut found = due;
            let floor = due.max(ms - window_ms);
            while candidate >= floor {
                let wall = utc(candidate)?.with_timezone(&zone).naive_local();
                if jeeves_agent::cron::matches(expression, wall)
                    && zone
                        .from_local_datetime(&wall)
                        .earliest()
                        .is_some_and(|t| t.timestamp_millis() == candidate)
                {
                    found = candidate;
                    break;
                }
                candidate -= 60_000;
            }
            found
        }
        _ => due,
    };
    let next = match trigger {
        Trigger::At { .. } => None,
        _ => first(trigger, utc(ms + 1)?)?,
    };
    Ok((selected, next))
}

pub(super) fn describe(trigger: &Trigger) -> String {
    match trigger {
        Trigger::At { at } => at.clone(),
        Trigger::After { delay_seconds } => format!("After confirmation + {delay_seconds} s"),
        Trigger::Every {
            interval_seconds, ..
        } => format!("Every {interval_seconds} s"),
        Trigger::Cron {
            expression,
            time_zone,
        } => {
            let fields: Vec<_> = expression.split_ascii_whitespace().collect();
            if let [minute, hour, "*", "*", days] = fields.as_slice() {
                if let (Ok(minute), Ok(hour)) = (minute.parse::<u8>(), hour.parse::<u8>()) {
                    let days: String = if *days == "*" {
                        "Every day".into()
                    } else {
                        let labels = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
                        days.chars()
                            .map(|c| {
                                c.to_digit(10)
                                    .and_then(|n| labels.get(n as usize))
                                    .map_or_else(|| c.to_string(), |day| (*day).into())
                            })
                            .collect()
                    };
                    return format!("{days} {hour:02}:{minute:02} {time_zone}");
                }
            }
            format!("Calendar schedule ({time_zone})")
        }
        Trigger::Event { source, event_type } => format!("{source} · {event_type}"),
    }
}

pub(super) fn local_zone() -> Result<Tz> {
    Ok(match jeeves_config::time_zone_override() {
        Some(zone) => zone.parse()?,
        None => iana_time_zone::get_timezone()?.parse()?,
    })
}

async fn initialize_imported_clock(server: &Server, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    let rows: Vec<(String, i32, String, Option<i64>)> = sqlx::query_as(
        "SELECT id,revision,plan_json,last_slot FROM responsibilities WHERE status='active' AND next_due_ms=0
         AND expires_ms>? ORDER BY id LIMIT 64",
    )
    .bind(now.timestamp_millis())
    .fetch_all(&server.pool)
    .await?;
    for (id, revision, json, last_slot) in rows {
        let mut plan: ResponsibilityPlan =
            serde_json::from_str(&json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        let Trigger::Cron { time_zone, .. } = &mut plan.trigger else {
            continue;
        };
        let zone = local_zone();
        let zone = match zone {
            Ok(zone) => zone,
            Err(error) => {
                let reason = format!("Confirm the schedule time zone before resuming: {error}");
                sqlx::query("UPDATE responsibilities SET status='paused',migration_note=?,revision=revision+1,next_due_ms=NULL WHERE id=? AND revision=? AND status='active' AND next_due_ms=0")
                    .bind(&reason).bind(&id).bind(revision).execute(&server.pool).await?;
                eprintln!("(jeeves) responsibility {id} paused: {reason}");
                continue;
            }
        };
        *time_zone = zone.to_string();
        let after = last_slot
            .and_then(|slot| slot.checked_mul(60_000))
            .and_then(DateTime::from_timestamp_millis)
            .map_or(now, |last| now.max(last));
        let due = first(&plan.trigger, after).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        sqlx::query("UPDATE responsibilities SET next_due_ms=?,plan_json=? WHERE id=? AND revision=? AND status='active' AND next_due_ms=0")
            .bind(due).bind(serde_json::to_string(&plan).map_err(|e| sqlx::Error::Decode(Box::new(e)))?)
            .bind(&id).bind(revision).execute(&server.pool).await?;
        eprintln!("(jeeves) responsibility {id} moved to the shared timer in {zone}");
    }
    Ok(())
}

pub(super) async fn tick(server: &Server, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    initialize_imported_clock(server, now).await?;
    let ms = now.timestamp_millis();
    let rows: Vec<(String, i32, i64, String, i64)> = sqlx::query_as(
        "SELECT id, revision, next_due_ms, plan_json, enabled_ms FROM responsibilities
         WHERE status='active' AND next_due_ms<=? AND expires_ms>? AND plan_json IS NOT NULL
         ORDER BY next_due_ms LIMIT 64",
    )
    .bind(ms)
    .bind(ms)
    .fetch_all(&server.pool)
    .await?;
    for (id, revision, due, json, enabled) in rows {
        let plan: ResponsibilityPlan =
            serde_json::from_str(&json).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        let window = CATCH_UP_MS;
        let (selected, next) = slot(&plan.trigger, due, now, window)
            .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
        let recent = selected >= enabled && ms - selected <= window;
        let claim = claim::Claim {
            id: &id,
            revision,
            due,
            selected,
            next,
            now: ms,
            recent,
        };
        if claim::admit(&server.pool, claim).await? {
            if recent {
                eprintln!("(jeeves) responsibility {id} admitted slot {selected}");
            } else {
                eprintln!("(jeeves) responsibility {id} skipped slot {selected}");
            }
        }
    }
    Ok(())
}

mod claim {
    use super::*;
    use jeeves_state::responsibility_runs::{admit_on, Occurrence};

    pub(super) struct Claim<'a> {
        pub id: &'a str,
        pub revision: i32,
        pub due: i64,
        pub selected: i64,
        pub next: Option<i64>,
        pub now: i64,
        pub recent: bool,
    }

    pub(super) async fn admit(pool: &SqlitePool, claim: Claim<'_>) -> Result<bool, sqlx::Error> {
        let Claim {
            id,
            revision,
            due,
            selected,
            next,
            now,
            recent,
        } = claim;
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let changed = sqlx::query(
            "UPDATE responsibilities SET next_due_ms=?,last_slot=? WHERE id=? AND revision=?
             AND status='active' AND expires_ms>? AND next_due_ms=?",
        )
        .bind(next)
        .bind(selected.div_euclid(60_000))
        .bind(id)
        .bind(revision)
        .bind(now)
        .bind(due)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            == 1;
        if changed {
            admit_on(
                &mut tx,
                Occurrence {
                    id,
                    key: &format!("responsibility:{id}:time:{selected}"),
                    revision,
                    due_ms: Some(selected),
                    data: None,
                    skip: (!recent).then_some("Outside the catch-up window or paused at this slot"),
                },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(changed)
    }
}
