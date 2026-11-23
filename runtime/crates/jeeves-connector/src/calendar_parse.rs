use anyhow::{Context, Result, bail};
use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Tz;
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime, Event};
use serde::Serialize;
use std::str::FromStr;

#[derive(Debug, Serialize)]
pub struct CalendarEvent {
    pub uid: String,
    pub summary: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: String,
    pub end: String,
    pub start_local: String,
    pub end_local: String,
    pub timezone: String,
    pub all_day: bool,
}

struct ParsedStart {
    utc: DateTime<Utc>,
    zone: Tz,
    all_day: bool,
}

pub fn events_in_range(
    ics: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    timezone: &str,
) -> Result<Vec<CalendarEvent>> {
    if start >= end {
        bail!("calendar range start must be before end");
    }
    let display_zone: Tz = timezone.parse().context("unknown IANA timezone")?;
    let calendar = Calendar::from_str(ics).map_err(|e| anyhow::anyhow!("invalid ICS: {e}"))?;
    let mut out = Vec::new();
    for component in &calendar.components {
        let Some(event) = component.as_event() else {
            continue;
        };
        if event.property_value("STATUS") == Some("CANCELLED") {
            continue;
        }
        let parsed = parse_event(event, display_zone)?;
        let duration = parsed.end - parsed.start.utc;
        if let Some(rule) = event.property_value("RRULE") {
            let recurrence_zone: rrule::Tz = parsed.start.zone.into();
            let dt_start = parsed.start.utc.with_timezone(&recurrence_zone);
            let rule: rrule::RRule<rrule::Unvalidated> = rule
                .parse()
                .map_err(|e| anyhow::anyhow!("invalid RRULE for {}: {e}", parsed.uid))?;
            let mut set = rrule::RRuleSet::new(dt_start)
                .rrule(
                    rule.validate(dt_start)
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?,
                )
                .after(start.with_timezone(&recurrence_zone))
                .before(end.with_timezone(&recurrence_zone));
            for property in event.multi_properties().get("EXDATE").into_iter().flatten() {
                if let Some(value) = DatePerhapsTime::from_property(property) {
                    let excluded = parse_start(value, display_zone)?.utc;
                    set = set.exdate(excluded.with_timezone(&recurrence_zone));
                }
            }
            for occurrence in set.all(500).dates {
                let at = occurrence.with_timezone(&Utc);
                out.push(event_view(&parsed, at, at + duration, display_zone));
            }
        } else if parsed.start.utc < end && (parsed.start.utc >= start || parsed.end > start) {
            out.push(event_view(
                &parsed,
                parsed.start.utc,
                parsed.end,
                display_zone,
            ));
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then(a.uid.cmp(&b.uid)));
    Ok(out)
}

struct ParsedEvent {
    uid: String,
    summary: String,
    description: Option<String>,
    location: Option<String>,
    start: ParsedStart,
    end: DateTime<Utc>,
}

fn parse_event(event: &Event, display_zone: Tz) -> Result<ParsedEvent> {
    let start_property = event
        .properties()
        .get("DTSTART")
        .context("VEVENT has no DTSTART")?;
    let start_value =
        DatePerhapsTime::from_property(start_property).context("VEVENT has an invalid DTSTART")?;
    let start = parse_start(start_value, display_zone)?;
    let end = match event.properties().get("DTEND") {
        Some(property) => {
            let value =
                DatePerhapsTime::from_property(property).context("VEVENT has an invalid DTEND")?;
            parse_start(value, display_zone)?.utc
        }
        None if start.all_day => start.utc + chrono::Duration::days(1),
        None => start.utc,
    };
    if end < start.utc {
        bail!("VEVENT DTEND precedes DTSTART");
    }
    Ok(ParsedEvent {
        uid: event
            .property_value("UID")
            .unwrap_or("(no uid)")
            .to_string(),
        summary: unescape_text(event.property_value("SUMMARY").unwrap_or("(untitled)")),
        description: event.property_value("DESCRIPTION").map(unescape_text),
        location: event.property_value("LOCATION").map(unescape_text),
        start,
        end,
    })
}

fn parse_start(value: DatePerhapsTime, display_zone: Tz) -> Result<ParsedStart> {
    let (date_time, zone, all_day) = match value {
        DatePerhapsTime::Date(date) => (
            date.and_hms_opt(0, 0, 0).expect("midnight"),
            display_zone,
            true,
        ),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(date_time)) => {
            (date_time, display_zone, false)
        }
        DatePerhapsTime::DateTime(CalendarDateTime::WithTimezone { date_time, tzid }) => {
            let zone = tzid
                .parse()
                .with_context(|| format!("unknown TZID {tzid}"))?;
            (date_time, zone, false)
        }
        DatePerhapsTime::DateTime(CalendarDateTime::Utc(utc)) => {
            return Ok(ParsedStart {
                utc,
                zone: chrono_tz::UTC,
                all_day: false,
            });
        }
    };
    let utc = match zone.from_local_datetime(&date_time) {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(first, _) => first.with_timezone(&Utc),
        LocalResult::None => bail!("local time {date_time} does not exist in {zone}"),
    };
    Ok(ParsedStart { utc, zone, all_day })
}

fn event_view(
    event: &ParsedEvent,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    display_zone: Tz,
) -> CalendarEvent {
    CalendarEvent {
        uid: event.uid.clone(),
        summary: event.summary.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: start.to_rfc3339(),
        end: end.to_rfc3339(),
        start_local: start.with_timezone(&display_zone).to_rfc3339(),
        end_local: end.with_timezone(&display_zone).to_rfc3339(),
        timezone: display_zone.name().to_string(),
        all_day: event.start.all_day,
    }
}

fn unescape_text(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
}

/// Extract CalDAV calendar-data without expanding general XML entities. CalDAV servers normally
/// use CDATA; plain text supports only predefined XML entities and scalar references.
pub fn calendar_documents(response: &str) -> Result<Vec<String>> {
    if response.to_ascii_lowercase().contains("<!doctype") {
        bail!("CalDAV XML with a DOCTYPE is refused");
    }
    let mut documents = Vec::new();
    let mut cursor = 0;
    let mut content_start = None;
    while let Some(relative) = response[cursor..].find('<') {
        let open = cursor + relative;
        let close = response[open..]
            .find('>')
            .map(|at| open + at)
            .context("unterminated CalDAV XML tag")?;
        let raw = response[open + 1..close].trim();
        if response[open..].starts_with("<![CDATA[") {
            let end = response[open..]
                .find("]]>")
                .map(|at| open + at + 3)
                .context("unterminated CDATA in CalDAV XML")?;
            cursor = end;
            continue;
        }
        if raw.starts_with('!') || raw.starts_with('?') {
            cursor = close + 1;
            continue;
        }
        let closing = raw.starts_with('/');
        let name = raw
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit(':')
            .next()
            .unwrap_or("");
        if name.eq_ignore_ascii_case("calendar-data") {
            if closing {
                let begin = content_start
                    .take()
                    .context("unexpected calendar-data close")?;
                let content = &response[begin..open];
                let document = if content.starts_with("<![CDATA[") && content.ends_with("]]>") {
                    content[9..content.len() - 3].to_string()
                } else {
                    decode_xml(content)?
                };
                documents.push(document);
            } else if !raw.ends_with('/') {
                content_start = Some(close + 1);
            }
        }
        cursor = close + 1;
    }
    if documents.is_empty() && response.contains("BEGIN:VCALENDAR") {
        documents.push(response.to_string());
    }
    if response.trim_start().starts_with('<') {
        for status in crate::caldav_discovery::elements(response, "status")? {
            let code = status
                .split_whitespace()
                .nth(1)
                .and_then(|s| s.parse::<u16>().ok());
            anyhow::ensure!(
                code.is_some_and(|code| (200..300).contains(&code)),
                "CalDAV report contained a failed item; cannot confirm a complete calendar read"
            );
        }
    }
    Ok(documents)
}

fn decode_xml(value: &str) -> Result<String> {
    let mut rest = value;
    let mut out = String::with_capacity(value.len());
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail
            .find(';')
            .context("unterminated XML entity in calendar-data")?;
        let entity = &tail[..end];
        let code = match entity {
            "lt" => Some('<' as u32),
            "gt" => Some('>' as u32),
            "quot" => Some('"' as u32),
            "apos" => Some('\'' as u32),
            "amp" => Some('&' as u32),
            _ => entity
                .strip_prefix("#x")
                .and_then(|n| u32::from_str_radix(n, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|n| n.parse().ok())),
        }
        .context("unsupported XML entity in CalDAV calendar-data")?;
        anyhow::ensure!(
            matches!(code,9|10|13|0x20..=0xd7ff|0xe000..=0xfffd|0x10000..=0x10ffff),
            "invalid XML scalar in calendar-data"
        );
        out.push(char::from_u32(code).expect("validated XML scalar"));
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}
