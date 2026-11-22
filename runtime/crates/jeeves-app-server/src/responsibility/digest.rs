//! Read selected evidence into a private digest at the occurrence's local calendar date.
use crate::Server;
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use jeeves_protocol::{CalendarWindow, DigestLayout, ResponsibilityDigest};
use serde::Deserialize;
use serde_json::json;

fn date_range(date: NaiveDate, window: CalendarWindow) -> Result<(NaiveDate, NaiveDate)> {
    let week = date.weekday().num_days_from_monday() as u64;
    let (first, count) = match window {
        CalendarWindow::Today => (Some(date), 1),
        CalendarWindow::Tomorrow => (date.succ_opt(), 1),
        CalendarWindow::ThisWeek => (date.checked_sub_days(Days::new(week)), 7),
        CalendarWindow::NextWeek => (date.checked_add_days(Days::new(7 - week)), 7),
    };
    let first = first.context("Date out of range")?;
    Ok((
        first,
        first
            .checked_add_days(Days::new(count))
            .context("Date out of range")?,
    ))
}

fn midnight(date: NaiveDate, zone: Tz) -> Result<DateTime<Tz>> {
    (0..1440)
        .find_map(|m| {
            date.and_hms_opt(m / 60, m % 60, 0)
                .and_then(|t| zone.from_local_datetime(&t).earliest())
        })
        .context("Local date is unavailable")
}

#[derive(Deserialize)]
struct Event {
    uid: String,
    summary: String,
    start: String,
}
#[derive(Deserialize)]
struct Calendar {
    events: Vec<Event>,
    total: usize,
}
#[derive(Deserialize)]
struct Mail {
    uid: u32,
    subject: Option<String>,
}
#[derive(Deserialize)]
struct Inbox {
    messages: Vec<Mail>,
}

async fn read<T: serde::de::DeserializeOwned>(
    server: &Server,
    name: &str,
    args: serde_json::Value,
) -> Result<T> {
    let settings = server.settings();
    let tools = crate::enabled_tools(
        &crate::connector_settings::apply(&settings.home, &server.tools),
        &settings,
    );
    ensure!(
        tools.effect(name).is_read(),
        "Read source {name} is unavailable"
    );
    let result = tools
        .execute_result(
            server.tool_ctx("responsibility:digest", None, None),
            name,
            args,
        )
        .await;
    ensure!(result.success, "Source {name} unavailable: {}", result.text);
    Ok(serde_json::from_str(&result.text)?)
}

pub(crate) async fn render(
    server: &Server,
    options: &ResponsibilityDigest,
    sources: &[String],
    at: DateTime<Utc>,
) -> Result<String> {
    use crate::scheduler::morning::plain;
    let zone: Tz = options.time_zone.parse()?;
    let date = at.with_timezone(&zone).date_naive();
    let mut sections = Vec::new();
    for source in sources {
        let (heading, mut items) = match source.as_str() {
            "calendar" => {
                let (start, end) = date_range(date, options.calendar_window)?;
                let rows:Calendar=read(server,"calendar_list",json!({"start":midnight(start,zone)?.to_rfc3339(),"end":midnight(end,zone)?.to_rfc3339(),"timezone":zone.name(),"limit":30})).await?;
                let mut items = rows
                    .events
                    .iter()
                    .map(|r| {
                        format!(
                            "{} {} (calendar {} @ {})",
                            plain(&r.start, 48),
                            plain(&r.summary, 128),
                            plain(&r.uid, 160),
                            plain(&r.start, 48)
                        )
                    })
                    .collect::<Vec<_>>();
                if rows.total > items.len() {
                    items.push(format!("… (+{})", rows.total - items.len()));
                }
                (format!("日历 / Calendar · {start} – {end} ({zone})"), items)
            }
            "openTasks" => {
                let rows = server.tasks.list_active().await?;
                let mut items = rows
                    .iter()
                    .take(20)
                    .map(|r| format!("{} (task {})", plain(&r.title, 128), plain(&r.id, 100)))
                    .collect::<Vec<_>>();
                if rows.len() > 20 {
                    items.push(format!("… (+{})", rows.len() - 20));
                }
                ("未完成事项 / Open tasks".into(), items)
            }
            "unreadMail" => {
                let rows: Inbox = read(server, "email_read", json!({"limit":8})).await?;
                (
                    "未读邮件 / Unread mail".into(),
                    rows.messages
                        .into_iter()
                        .map(|m| {
                            format!(
                                "{} (INBOX UID {})",
                                plain(m.subject.as_deref().unwrap_or("(unavailable)"), 128),
                                m.uid
                            )
                        })
                        .collect(),
                )
            }
            _ => anyhow::bail!("Unknown digest source {source}"),
        };
        if items.is_empty() {
            items.push("无匹配记录 / No matches".into());
        }
        sections.push(match options.layout {
            DigestLayout::Paragraphs => format!("{heading} — {}", items.join(" · ")),
            DigestLayout::List => format!("{heading}\n- {}", items.join("\n- ")),
        });
    }
    Ok(sections.join("\n\n"))
}
