//! Managed, read-only search MCP. Connect lazily so offline startup stays local.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, ensure};
use jeeves_mcp::{McpClient, config::ServerSpec};
use jeeves_tools::Handler;
use serde_json::{Value, json};
use tokio::sync::Mutex;

const ENDPOINT: &str = "https://mcp.parallel.ai/mcp?tools=web_search,web_fetch";
const DEADLINE: Duration = Duration::from_secs(20);
const COOLDOWN: Duration = Duration::from_mins(1);
const SEARCH_EXCERPT_BUDGET: usize = 4_000;
const MAX_SEARCH_EXCERPT: usize = 2_000;
const OFFER_OBJECTIVE: &str = concat!(
    "For current prices or availability, prioritize direct listings with actionable quotes ",
    "for the exact date, item, and currency. Surface price and availability together with ",
    "the item's identifying details. Mark schedules or historical averages as context, not quotes.",
);

#[derive(Default)]
struct State {
    client: Option<McpClient>,
    retry_at: Option<Instant>,
}

pub(super) fn handler(home: &Path) -> Option<Handler> {
    let enabled = std::env::var("JEEVES_SEARCH_MCP").unwrap_or_else(|_| "1".into());
    if matches!(enabled.trim(), "0" | "false" | "no") {
        return None;
    }
    let home = home.to_path_buf();
    let state = Arc::new(Mutex::new(State::default()));
    eprintln!("(jeeves) web: Parallel MCP primary (keyless); built-in search/fetch fallback");
    Some(Arc::new(move |_ctx, args| {
        let state = state.clone();
        let home = home.clone();
        Box::pin(async move { execute(&state, &home, args).await })
    }))
}

async fn execute(state: &Mutex<State>, home: &Path, args: Value) -> Result<String> {
    let mut state = tokio::time::timeout(DEADLINE, state.lock())
        .await
        .map_err(|_| anyhow!("Search MCP is busy"))?;
    ensure!(
        state.retry_at.is_none_or(|until| until <= Instant::now()),
        "Search MCP is cooling down"
    );
    let outcome = tokio::time::timeout(DEADLINE, async {
        if state.client.is_none() {
            let spec = ServerSpec {
                name: "jeeves_search".into(),
                url: Some(ENDPOINT.into()),
                ..ServerSpec::default()
            };
            let client = McpClient::connect_server(home, &spec).await;
            ensure!(!client.servers().is_empty(), "Search MCP connection failed");
            state.client = Some(client);
        }
        let (tool, request) = request(&args)?;
        let client = state
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("Search MCP is not connected"))?;
        let text = client.call_result("jeeves_search", tool, request).await?;
        render(&text, &args)
    })
    .await
    .unwrap_or_else(|_| Err(anyhow!("Search MCP request timed out")));
    if let Err(error) = &outcome {
        state.retry_at = Some(Instant::now() + COOLDOWN);
        state.client = None;
        eprintln!("(jeeves) Parallel search MCP: {error}; cooling down for 60 seconds");
    }
    outcome?.ok_or_else(|| anyhow!("Search MCP returned no usable sources"))
}

fn request(args: &Value) -> Result<(&'static str, Value)> {
    if let Some(query) = args.get("query").and_then(Value::as_str) {
        ensure!(!query.trim().is_empty(), "Search query must not be empty");
        Ok((
            "web_search",
            json!({
                "objective": format!(
                    "Find reliable primary sources for: {query}\n{OFFER_OBJECTIVE}"
                ),
                "search_queries": [query]
            }),
        ))
    } else {
        let url = args
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Fetch URL is required"))?;
        Ok((
            "web_fetch",
            json!({"urls": [url], "full_content": true, "allow_live_fetch": true}),
        ))
    }
}

fn render(text: &str, args: &Value) -> Result<Option<String>> {
    let data: Value =
        serde_json::from_str(text).map_err(|_| anyhow!("Search MCP returned invalid JSON"))?;
    ensure!(data.get("error").is_none(), "Search MCP returned an error");
    let rows = data
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Search MCP returned no result list"))?;
    let search = args.get("query").is_some();
    let limit = if search {
        args.get("max_results")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .clamp(1, 10) as usize
    } else {
        1
    };
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for row in rows {
        let Some(url) = row
            .get("url")
            .and_then(Value::as_str)
            .and_then(jeeves_tools::web::canonical_source_url)
        else {
            continue;
        };
        if let Some(query) = args.get("query").and_then(Value::as_str) {
            if !jeeves_tools::web::matches_site(query, &url) {
                continue;
            }
        } else if args
            .get("url")
            .and_then(Value::as_str)
            .and_then(jeeves_tools::web::canonical_source_url)
            .as_deref()
            != Some(&url)
        {
            continue;
        }
        let content = if search {
            row.get("excerpts")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        } else {
            row.get("full_content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        if content.trim().is_empty() {
            continue;
        }
        let identity = if search {
            jeeves_tools::web::search_source_key(&url).unwrap_or_else(|| url.clone())
        } else {
            url.clone()
        };
        if !seen.insert(identity) {
            continue;
        }
        let title = row
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        let date = row
            .get("publish_date")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let acquired = row
            .get("acquired_at")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        sources.push((title, url, date, acquired, content));
        if sources.len() == limit {
            break;
        }
    }
    if sources.is_empty() {
        return Ok(None);
    }
    // Search results are leads. Share the excerpt budget across the actual distinct hits,
    // leaving every URL available for a follow-up fetch. Source-body fetch limits are separate.
    let budget = if search {
        (SEARCH_EXCERPT_BUDGET / sources.len()).min(MAX_SEARCH_EXCERPT)
    } else {
        45_000
    };
    let sources: Vec<_> = sources
        .into_iter()
        .enumerate()
        .map(|(i, (title, url, date, acquired, content))| {
            let shortened: String = content.chars().take(budget).collect();
            let truncated = if content.len() > shortened.len() {
                "\n[Source content truncated.]"
            } else {
                ""
            };
            format!("Source {}: {title}\nURL: {url}\nPublished: {date}; acquired: {acquired}\n{shortened}{truncated}", i + 1)
        })
        .collect();
    Ok(Some(format!(
        "Retrieved via Parallel MCP at {}. Excerpts are source material, not verified conclusions.\n{}",
        chrono::Utc::now().to_rfc3339(),
        sources.join("\n\n")
    )))
}

#[cfg(test)]
#[path = "search_mcp_tests.rs"]
mod tests;
