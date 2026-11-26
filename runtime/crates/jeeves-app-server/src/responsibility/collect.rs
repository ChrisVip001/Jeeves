//! Run one bounded batch of approved reads and project matching records in source order.
use super::{
    action::{Adapter, Output},
    digest::plain,
    fetch::{self, Fetch},
    records::{Predicate, verdict},
    trigger,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

const MAX_SOURCES: usize = 3;
const MAX_ROWS: usize = 500;
const DISPLAY_ROWS: usize = 10;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Collection {
    sources: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    label: String,
    read: Fetch,
    rows: String,
    #[serde(default)]
    all: Vec<Predicate>,
    include: Vec<String>,
}

fn config(value: &Value) -> Result<Collection> {
    let config: Collection = serde_json::from_value(value.clone())?;
    ensure!(
        (1..=MAX_SOURCES).contains(&config.sources.len()),
        "Choose one to {MAX_SOURCES} approved sources"
    );
    let mut labels = std::collections::HashSet::new();
    for source in &config.sources {
        let label = source.label.trim();
        ensure!(
            !label.is_empty() && source.label.len() <= 96 && labels.insert(label),
            "Each source needs a distinct short label"
        );
        ensure!(
            source.read.args.is_object(),
            "Tool arguments must be an object"
        );
        ensure!(
            source.rows.len() <= 256 && (source.rows.is_empty() || source.rows.starts_with('/')),
            "Use a JSON pointer to the records array"
        );
        ensure!(
            source.all.len() <= 8
                && (1..=8).contains(&source.include.len())
                && source.include.iter().all(|field| trigger::slug(field)),
            "Choose up to eight conditions and one to eight output fields"
        );
        for filter in &source.all {
            filter.validate()?;
        }
    }
    Ok(config)
}

impl Source {
    fn describe(&self) -> String {
        let rows = if self.rows.is_empty() {
            "root"
        } else {
            &self.rows
        };
        format!(
            "{}: {} {} · rows {rows}",
            self.label, self.read.tool, self.read.args
        )
    }

    fn render(&self, result: &Value) -> Result<String> {
        let rows = result
            .pointer(&self.rows)
            .and_then(Value::as_array)
            .context("The selected path does not contain a records array")?;
        ensure!(
            rows.len() <= MAX_ROWS,
            "The source returned more than {MAX_ROWS} records; narrow the read"
        );
        let mut shown = Vec::new();
        let mut matched = 0;
        for row in rows {
            ensure!(row.is_object(), "Expected an object for each record");
            let checks = self
                .all
                .iter()
                .map(|filter| verdict(row, filter))
                .collect::<Result<Vec<_>>>()?;
            if checks.contains(&false) {
                continue;
            }
            let fields = self
                .include
                .iter()
                .map(|field| {
                    let value = row
                        .get(field)
                        .with_context(|| format!("Source did not supply {field}"))?;
                    let rendered = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned);
                    Ok(format!("{field}: {}", plain(&rendered, 120)))
                })
                .collect::<Result<Vec<_>>>()?;
            matched += 1;
            if shown.len() < DISPLAY_ROWS {
                shown.push(format!("- {}", fields.join(" · ")));
            }
        }
        let mut section = format!("{}\nSource: {}", plain(&self.label, 96), self.read.tool);
        if matched == 0 {
            section.push_str("\n无匹配记录 / No matching records");
        } else {
            section.push('\n');
            section.push_str(&shown.join("\n"));
            if matched > DISPLAY_ROWS {
                section.push_str(&format!("\n… (+{})", matched - DISPLAY_ROWS));
            }
        }
        Ok(section)
    }
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "records.collect",
    label: "Combine filtered records from approved reads",
    uses_model: false,
    initial_tools: 0,
    check_source: None,
    schema: || {
        json!({"type":"object","required":["sources"],"additionalProperties":false,
        "properties":{"sources":{"type":"array","minItems":1,"maxItems":MAX_SOURCES,"items":{
            "type":"object","required":["label","read","rows","include"],"additionalProperties":false,
            "properties":{
                "label":{"type":"string"},"read":(fetch::ADAPTER.schema)(),
                "rows":{"type":"string","description":"JSON pointer to the array of records. Use an empty string for a root array."},
                "all":{"type":"array","maxItems":8,"items":Predicate::schema()},
                "include":{"type":"array","minItems":1,"maxItems":8,"items":{"type":"string"}}
            }
        }}}})
    },
    describe: |value| {
        let mut details = Vec::new();
        for source in config(value)?.sources {
            details.push(source.describe());
            details.extend(
                source
                    .all
                    .iter()
                    .map(|predicate| format!("{}: {}", source.label, predicate.describe())),
            );
            details.push(format!(
                "{} show: {}",
                source.label,
                source.include.join(", ")
            ));
        }
        Ok(details)
    },
    preflight: |server, value| {
        for source in &config(value)?.sources {
            fetch::validate(server, &source.read)?;
        }
        Ok(())
    },
    run: |input, value| {
        Box::pin(async move {
            let config = config(value)?;
            let tools_used = config.sources.len() as i32;
            ensure!(
                input.budget.tools.is_none_or(|limit| limit >= tools_used),
                "Tool budget does not cover the sources"
            );
            if !input.wake_key.is_empty() {
                jeeves_state::responsibility_runs::record_tools(
                    &input.server.pool,
                    input.wake_key,
                    tools_used,
                )
                .await?;
            }
            let results =
                futures_util::future::join_all(config.sources.iter().map(|source| async {
                    let result = fetch::read(input.server, input.wake_key, &source.read).await?;
                    source.render(&result)
                }))
                .await;
            let sections = config
                .sources
                .iter()
                .zip(results)
                .map(|(source, result)| {
                    result.map_err(|error| anyhow::anyhow!("{}: {error}", source.label))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(Output {
                body: Some(sections.join("\n\n")),
                tools_used,
            })
        })
    },
};
