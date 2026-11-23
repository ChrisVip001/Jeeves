#[path = "calendar_parse.rs"]
mod calendar_parse;
pub use calendar_parse::{CalendarEvent, calendar_documents, events_in_range};

use crate::config::CalendarConfig;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use jeeves_tools::{Tool, ToolCtx, require_str};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

pub fn calendar_list_tool(config: CalendarConfig) -> Tool {
    Tool {
        name: "calendar_list".into(),
        description: "Read calendar events from the configured CalDAV collection or ICS feed. Expands recurring events with IANA timezones. Read-only.".into(),
        input_schema: json!({
            "type":"object",
            "properties": {
                "start":{"type":"string","description":"RFC3339 range start"},
                "end":{"type":"string","description":"RFC3339 range end"},
                "timezone":{"type":"string","description":"IANA timezone for display and floating/all-day events; defaults to the configured timezone"},
                "limit":{"type":"integer","minimum":1,"maximum":200}
            },
            "required":["start","end"],
            "additionalProperties":false
        }),
        handler: Arc::new(move |ctx, args| {
            let config = config.clone();
            Box::pin(async move { run(ctx, config, args).await })
        }),
    }
}

async fn run(ctx: ToolCtx, config: CalendarConfig, args: Value) -> Result<String> {
    let start = parse_time(&require_str(&args, "start")?)?;
    let end = parse_time(&require_str(&args, "end")?)?;
    let timezone = args
        .get("timezone")
        .and_then(Value::as_str)
        .unwrap_or(&config.timezone);
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
    let mut events = read_events(&ctx.home, &config, start, end, timezone).await?;
    let total = events.len();
    events.truncate(limit);
    Ok(serde_json::to_string_pretty(
        &json!({"events":events,"total":total,"truncated":total>limit}),
    )?)
}

/// Shared read path for the tool and human connection diagnostics.
pub async fn read_events(
    home: &std::path::Path,
    config: &CalendarConfig,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    timezone: &str,
) -> Result<Vec<CalendarEvent>> {
    let documents = fetch_documents(home, config, start, end).await?;
    let mut events = Vec::new();
    for document in documents {
        events.extend(events_in_range(&document, start, end, timezone)?);
    }
    events.sort_by(|a, b| a.start.cmp(&b.start).then(a.uid.cmp(&b.uid)));
    events.dedup_by(|a, b| a.uid == b.uid && a.start == b.start);
    Ok(events)
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .with_context(|| format!("expected an RFC3339 timestamp, got {value:?}"))?
        .with_timezone(&Utc))
}

async fn fetch_documents(
    home: &std::path::Path,
    config: &CalendarConfig,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<String>> {
    let url = reqwest::Url::parse(&config.url).context("invalid calendar URL")?;
    let secret = config
        .username
        .as_ref()
        .map(|_| {
            config
                .secret(home)
                .context("calendar password was not found at its configured pointer")
        })
        .transpose()?;
    let auth = config.username.as_deref().zip(secret.as_deref());
    if let Some((username, secret)) = auth
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none()
    {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("jeeves/", env!("CARGO_PKG_VERSION")))
            .build()?;
        return tokio::time::timeout(Duration::from_secs(50), async {
            let urls = crate::caldav_discovery::discover(&client, &url, username, secret).await?;
            let mut documents = Vec::new();
            let mut bytes = 0;
            for url in urls {
                for document in fetch_documents_at(&url, auth, start, end, &client).await? {
                    bytes += document.len();
                    anyhow::ensure!(
                        bytes <= MAX_RESPONSE_BYTES,
                        "combined calendar response exceeds the 10 MiB limit"
                    );
                    documents.push(document);
                }
            }
            Ok(documents)
        })
        .await
        .context("CalDAV service discovery/query timed out")?;
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("jeeves/", env!("CARGO_PKG_VERSION")))
        .build()?;
    fetch_documents_at(&url, auth, start, end, &client).await
}

async fn fetch_documents_at(
    url: &reqwest::Url,
    auth: Option<(&str, &str)>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    client: &reqwest::Client,
) -> Result<Vec<String>> {
    let direct_ics = url.path().ends_with(".ics");
    let request = if direct_ics {
        client.get(url.clone())
    } else {
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{}" end="{}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#,
            start.format("%Y%m%dT%H%M%SZ"),
            end.format("%Y%m%dT%H%M%SZ")
        );
        client
            .request(reqwest::Method::from_bytes(b"REPORT")?, url.clone())
            .header("Depth", "1")
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
    };
    let request = if let Some((username, secret)) = auth {
        request.basic_auth(username, Some(secret))
    } else {
        request
    };
    let response = request.send().await.context("calendar request failed")?;
    if !response.status().is_success() {
        bail!("calendar server returned HTTP {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        bail!("calendar response exceeds the 10 MiB limit");
    }
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("calendar response read failed")?;
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!("calendar response exceeds the 10 MiB limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(bytes).context("calendar response is not UTF-8")?;
    if direct_ics {
        Ok(vec![text])
    } else {
        calendar_documents(&text)
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
