//! Read selected evidence into a private digest at the occurrence's local calendar date.
use crate::Server;
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use jeeves_protocol::{CalendarWindow, DigestLayout, ResponsibilityDigest};
use serde::Deserialize;
use serde_json::{json, Value};

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

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Source {
    pub(super) label: String,
    #[serde(flatten)]
    pub(super) fetch: super::fetch::Fetch,
}

pub(super) fn validate(sources: &[String], options: &ResponsibilityDigest) -> Result<()> {
    let _: Tz = options.time_zone.parse()?;
    let mut seen = std::collections::HashSet::new();
    ensure!(
        !sources.is_empty()
            && sources.iter().all(|source| {
                matches!(source.as_str(), "calendar" | "openTasks" | "unreadMail")
                    && seen.insert(source)
            }),
        "Select distinct digest sources"
    );
    Ok(())
}

pub(super) fn validate_generic(sources: &[Source], options: &ResponsibilityDigest) -> Result<()> {
    let _: Tz = options.time_zone.parse()?;
    ensure!(
        (1..=8).contains(&sources.len()),
        "Choose one to eight sources"
    );
    for source in sources {
        ensure!(
            !source.label.trim().is_empty() && source.label.len() <= 96,
            "Each source needs a short label"
        );
    }
    Ok(())
}

pub(super) fn preflight(server: &Server, sources: &[String]) -> Result<()> {
    let settings = server.settings();
    let tools = crate::enabled_tools(
        &crate::connector_settings::apply(&settings.home, &server.tools),
        &settings,
    );
    for source in sources {
        let name = match source.as_str() {
            "calendar" => "calendar_list",
            "unreadMail" => "email_read",
            _ => continue,
        };
        ensure!(
            tools.effect(name).is_read(),
            "Source {source} is unavailable"
        );
    }
    Ok(())
}

pub(super) fn preflight_generic(server: &Server, sources: &[Source]) -> Result<()> {
    for source in sources {
        super::fetch::validate(server, &source.fetch)?;
    }
    Ok(())
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

pub(super) async fn render_generic(
    server: &Server,
    options: &ResponsibilityDigest,
    sources: &[Source],
    wake_key: &str,
) -> Result<String> {
    let mut sections = Vec::new();
    for source in sources {
        let record = super::fetch::read(server, wake_key, &source.fetch).await?;
        let items = record_items(&record);
        let items = if items.is_empty() {
            vec!["无匹配记录 / No matches".into()]
        } else {
            items
        };
        sections.push(match options.layout {
            DigestLayout::Paragraphs => format!("{} — {}", source.label, items.join(" · ")),
            DigestLayout::List => format!("{}\n- {}", source.label, items.join("\n- ")),
        });
    }
    Ok(sections.join("\n\n"))
}

fn record_items(record: &Value) -> Vec<String> {
    let render = |v: &Value| match v {
        Value::String(s) => plain(s, 160),
        _ => plain(&v.to_string(), 160),
    };
    let arrays: Vec<&Vec<Value>> = record
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .filter_map(Value::as_array)
        .take(1)
        .collect();
    if let Some(items) = arrays.first() {
        return items.iter().take(30).map(render).collect();
    }
    if let Some(map) = record.as_object() {
        return map
            .iter()
            .take(30)
            .map(|(k, v)| format!("{k}: {}", render(v)))
            .collect();
    }
    vec![render(record)]
}

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
