//! The opt-in morning duty builds a sourced proposal without a model or mutation tools.
use super::{Delivery, Job};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use jeeves_state::TaskStatus;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

pub(super) const NAME: &str = "daily-morning-briefing";
const MAX_EVENTS: usize = 30;
const MAX_TASKS: usize = 20;

#[derive(Clone, Copy)]
pub(super) struct Briefing {
    pub(super) zone: Tz,
    delivery: Delivery,
}

impl Briefing {
    pub(super) fn parse(raw: &str) -> Result<Self> {
        let (channel, zone) = raw
            .trim()
            .split_once('@')
            .context("expected outbox@Area/City or notify@Area/City")?;
        let delivery = match channel {
            "outbox" => Delivery::Outbox,
            "notify" => Delivery::Notify,
            _ => anyhow::bail!("channel must be outbox or notify"),
        };
        Ok(Self {
            zone: zone.parse().map_err(|_| anyhow!("unknown IANA timezone"))?,
            delivery,
        })
    }

    pub(super) fn job(self) -> Job {
        Job {
            name: NAME.into(),
            cron: "0 8 * * *".into(),
            prompt: "Produce a sourced morning proposal; never send email or mutate tasks.".into(),
            catch_up: true,
            delivery: self.delivery,
        }
    }
}

/// M2-03 reconciles the local clock of each job; ordinary jobs keep the host clock.
pub(super) fn wall(instant: DateTime<Utc>, zone: Option<Tz>) -> NaiveDateTime {
    match zone {
        Some(zone) => instant.with_timezone(&zone).naive_local(),
        None => instant.with_timezone(&Local).naive_local(),
    }
}

fn bounds(date: NaiveDate, zone: Tz) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let first = |date: NaiveDate| {
        (0..24 * 60)
            .filter_map(|minute| date.and_hms_opt(minute / 60, minute % 60, 0))
            .find_map(|time| zone.from_local_datetime(&time).earliest())
            .map(|time| time.with_timezone(&Utc))
    };
    let next = date.succ_opt().context("briefing date has no successor")?;
    let end = first(next).or_else(|| next.succ_opt().and_then(first));
    Ok((
        first(date).context("local date does not exist")?,
        end.context("next local date does not exist")?,
    ))
}

#[derive(Deserialize)]
struct Calendar {
    events: Vec<CalendarItem>,
    total: usize,
}

#[derive(Deserialize)]
struct CalendarItem {
    summary: String,
    start: String,
    all_day: bool,
}

#[derive(Deserialize)]
struct Inbox {
    messages: Vec<Unread>,
}

#[derive(Deserialize)]
struct Unread {
    subject: Option<String>,
    from: Option<String>,
    body: Option<String>,
}

struct TaskItem {
    id: String,
    title: String,
    status: TaskStatus,
    progress: Option<String>,
}

async fn calendar(
    config: jeeves_connector::config::CalendarConfig,
    ctx: jeeves_tools::ToolCtx,
    date: NaiveDate,
    zone: Tz,
) -> Result<Calendar> {
    let (start, end) = bounds(date, zone)?;
    let read = jeeves_connector::calendar::calendar_list_tool(config);
    let text = (read.handler)(
        ctx,
        json!({
            "start": start.to_rfc3339(), "end": end.to_rfc3339(),
            "timezone": zone.name(), "limit": MAX_EVENTS
        }),
    )
    .await?;
    Ok(serde_json::from_str(&text)?)
}

async fn mail(
    config: jeeves_connector::config::EmailConfig,
    ctx: jeeves_tools::ToolCtx,
) -> Result<Inbox> {
    let read = jeeves_connector::email::email_read_tool(config);
    let text = tokio::time::timeout(
        Duration::from_secs(25),
        (read.handler)(ctx, json!({"limit": 8})),
    )
    .await??;
    Ok(serde_json::from_str(&text)?)
}

async fn timeline(server: &crate::Server) -> Result<(Vec<TaskItem>, usize)> {
    let active = server.tasks.list_active().await?;
    let total = active.len();
    let mut result = Vec::with_capacity(total.min(MAX_TASKS));
    for task in active.into_iter().take(MAX_TASKS) {
        let progress = match server.events.list(&task.id, 1).await {
            Ok(rows) => rows.into_iter().next().map(|row| {
                let detail = row
                    .detail
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_default();
                format!("{} {} {}", row.at, row.kind, detail)
            }),
            Err(_) => Some("latest timeline event unavailable".into()),
        };
        result.push(TaskItem {
            id: task.id,
            title: task.title,
            status: task.status,
            progress,
        });
    }
    Ok((result, total))
}

pub(super) async fn proposal(server: &crate::Server, date: NaiveDate, zone: Tz) -> Result<String> {
    let config = jeeves_connector::config::load(&server.settings().home);
    let ctx = server.tool_ctx("schedule:daily-morning-briefing", None, None);
    let read_calendar = async {
        let conf = config
            .as_ref()
            .map_err(|_| anyhow!("connector configuration unavailable"))?;
        let Some(conf) = conf.calendar.clone() else {
            return Ok(None);
        };
        calendar(conf, ctx.clone(), date, zone).await.map(Some)
    };
    let read_mail = async {
        let conf = config
            .as_ref()
            .map_err(|_| anyhow!("connector configuration unavailable"))?;
        let Some(conf) = conf.email.clone() else {
            return Ok(None);
        };
        mail(conf, ctx.clone()).await.map(Some)
    };
    let (cal, inbox, tasks) = tokio::join!(read_calendar, read_mail, timeline(server));
    render(date, zone, cal, inbox, tasks)
}

/// Source text is data, not instructions; keep heading and channel markup under Runtime control.
pub(crate) fn plain(text: &str, max: usize) -> String {
    let words = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = words
        .chars()
        .take(max)
        .map(|c| match c {
            '<' => '‹',
            '>' => '›',
            '[' => '［',
            ']' => '］',
            '\u{60}' => '＇',
            '*' => '＊',
            '_' => '＿',
            '&' => '＆',
            '@' => '＠',
            other if other.is_control() => ' ',
            other => other,
        })
        .collect();
    if words.chars().nth(max).is_some() {
        out.push('…');
    }
    out
}

fn render(
    date: NaiveDate,
    zone: Tz,
    calendar: Result<Option<Calendar>>,
    inbox: Result<Option<Inbox>>,
    tasks: Result<(Vec<TaskItem>, usize)>,
) -> Result<String> {
    let mut out = format!("# Morning briefing proposal — {date} ({zone})\n\nRead-only; replies below are drafts.\n\n## Today's calendar\n");
    match calendar {
        Ok(Some(calendar)) if calendar.events.is_empty() => out.push_str("- No events today.\n"),
        Ok(Some(calendar)) => {
            for item in &calendar.events {
                let start = DateTime::parse_from_rfc3339(&item.start)
                    .context("calendar event has an invalid start timestamp")?
                    .with_timezone(&zone);
                anyhow::ensure!(
                    start.date_naive() <= date,
                    "calendar event starts after the briefing date"
                );
                let at = if item.all_day {
                    "All day".into()
                } else if start.date_naive() < date {
                    "Ongoing".into()
                } else {
                    start.format("%H:%M").to_string()
                };
                out.push_str(&format!("- {at} — {}\n", plain(&item.summary, 96)));
            }
            add_omitted(&mut out, calendar.total, calendar.events.len());
        }
        Ok(None) => out.push_str("- Calendar unavailable: not configured.\n"),
        Err(_) => out.push_str("- Calendar unavailable: source could not be read.\n"),
    }
    out.push_str("\n## Unfinished tasks and latest progress\n");
    match tasks {
        Ok((tasks, _)) if tasks.is_empty() => out.push_str("- No unfinished tasks.\n"),
        Ok((tasks, total)) => {
            for task in &tasks {
                anyhow::ensure!(
                    task.status.is_open(),
                    "briefing acceptance: source returned a terminal task"
                );
                let id = plain(&task.id, 72);
                let title = plain(&task.title, 96);
                anyhow::ensure!(
                    !id.is_empty() && !title.is_empty(),
                    "briefing acceptance: task has no source identity"
                );
                let progress = task
                    .progress
                    .as_ref()
                    .map_or_else(|| "no timeline event yet".into(), |text| plain(text, 100));
                out.push_str(&format!(
                    "- {title} ({}, {id}): {progress}\n",
                    task.status.as_str()
                ));
            }
            add_omitted(&mut out, total, tasks.len());
        }
        Err(_) => out.push_str("- Tasks unavailable: timeline could not be read.\n"),
    }
    out.push_str("\n## Recent unread mail and reply drafts\n");
    match inbox {
        Ok(Some(inbox)) if inbox.messages.is_empty() => out.push_str("- No unread mail.\n"),
        Ok(Some(inbox)) => {
            for row in inbox.messages {
                let Some(subject) = row.subject else {
                    out.push_str("- One unread message could not be parsed.\n");
                    continue;
                };
                let subject = plain(&subject, 100);
                let from = plain(row.from.as_deref().unwrap_or("unknown sender"), 65);
                let excerpt = plain(row.body.as_deref().unwrap_or(""), 150);
                let excerpt = if excerpt.is_empty() {
                    "(no plain-text preview)"
                } else {
                    &excerpt
                };
                out.push_str(&format!("- {subject} — {from}: {excerpt}\n  - Draft reply: Re: {subject} — Thanks for your message. I'll review it and get back to you.\n"));
            }
        }
        Ok(None) => out.push_str("- Unread mail unavailable: not configured.\n"),
        Err(_) => out.push_str("- Unread mail unavailable: source could not be read.\n"),
    }
    Ok(out)
}

fn add_omitted(out: &mut String, total: usize, displayed: usize) {
    let omitted = total.saturating_sub(displayed);
    if omitted > 0 {
        out.push_str(&format!("- {omitted} more from the source.\n"));
    }
}

#[cfg(test)]
mod tests;
