//! Evaluate owner-reviewed predicates over structured records without model calls.
use super::{
    action::{Adapter, Output},
    events, trigger,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Conditions {
    all: Vec<Predicate>,
    include: Vec<String>,
    message: String,
    snapshot: Option<Snapshot>,
    source: Option<super::fetch::Fetch>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    source: String,
    event_type: String,
    max_age_seconds: u32,
}

impl Snapshot {
    fn trigger(&self) -> jeeves_protocol::ResponsibilityTrigger {
        jeeves_protocol::ResponsibilityTrigger::Event {
            source: self.source.clone(),
            event_type: self.event_type.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Predicate {
    field: String,
    op: Operator,
    value: Value,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Operator {
    Eq,
    In,
    Contains,
    Lt,
    Gt,
}

fn config(value: &Value) -> Result<Conditions> {
    let config: Conditions = serde_json::from_value(value.clone())?;
    let sources = usize::from(config.snapshot.is_some()) + usize::from(config.source.is_some());
    ensure!(sources <= 1, "Choose one record source");
    ensure!(
        (1..=8).contains(&config.all.len()) && config.include.len() <= 8,
        "Choose up to eight conditions and output fields"
    );
    ensure!(
        !config.message.trim().is_empty() && config.message.len() <= 1024,
        "Supply a message up to 1024 bytes"
    );
    ensure!(
        config.include.iter().all(|field| trigger::slug(field)),
        "Output fields must be declared field names"
    );
    for filter in &config.all {
        ensure!(
            trigger::slug(&filter.field),
            "Conditions must reference declared field names"
        );
        let simple = |v: &Value| v.is_string() || v.is_boolean() || v.as_i64().is_some();
        let valid = match filter.op {
            Operator::Eq | Operator::Contains => simple(&filter.value),
            Operator::In => filter
                .value
                .as_array()
                .is_some_and(|items| (1..=32).contains(&items.len()) && items.iter().all(simple)),
            Operator::Lt | Operator::Gt => filter.value.as_i64().is_some(),
        };
        ensure!(valid, "Invalid comparison value for {}", filter.field);
    }
    if let Some(source) = &config.snapshot {
        ensure!(
            trigger::slug(&source.source)
                && trigger::slug(&source.event_type)
                && (60..=86_400).contains(&source.max_age_seconds),
            "Choose a configured source and a freshness limit from 60 seconds to one day"
        );
    }
    Ok(config)
}

pub(super) fn less_than(value: &Value, limit: &Value) -> bool {
    value
        .as_i64()
        .zip(limit.as_i64())
        .is_some_and(|(value, limit)| value < limit)
}

fn verdict(record: &Value, filter: &Predicate) -> Result<bool> {
    let value = record
        .get(&filter.field)
        .with_context(|| format!("Source did not supply {}", filter.field))?;
    let matches = match filter.op {
        Operator::Eq => value == &filter.value,
        Operator::In => filter
            .value
            .as_array()
            .is_some_and(|choices| choices.contains(value)),
        Operator::Contains => value
            .as_array()
            .is_some_and(|items| items.contains(&filter.value)),
        Operator::Lt => less_than(value, &filter.value),
        Operator::Gt => less_than(&filter.value, value),
    };
    Ok(matches)
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "records.evaluate",
    label: "Check conditions on source records",
    uses_model: false,
    initial_tools: 0,
    check_source: Some(|server, value, fields| {
        use events::Field;
        let config = config(value)?;
        if config.source.is_some() {
            return Ok(());
        }
        let snapshot_fields = match &config.snapshot {
            Some(source) => {
                ensure!(
                    fields.is_none(),
                    "Use the incoming event or a scheduled snapshot"
                );
                events::preflight(server, &source.trigger())?
            }
            None => None,
        };
        let fields = fields
            .or(snapshot_fields.as_ref())
            .context("Choose a signed event source for this record action")?;
        for rule in config.all {
            let field = fields
                .get(&rule.field)
                .with_context(|| format!("Source does not declare {}", rule.field))?;
            let scalar = |value: &Value| match field {
                Field::String | Field::StringArray => value.is_string(),
                Field::Integer => value.as_i64().is_some(),
                Field::Boolean => value.is_boolean(),
            };
            let valid = match rule.op {
                Operator::Contains => *field == Field::StringArray && scalar(&rule.value),
                Operator::Lt | Operator::Gt => *field == Field::Integer,
                Operator::Eq => *field != Field::StringArray && scalar(&rule.value),
                Operator::In => {
                    *field != Field::StringArray
                        && rule
                            .value
                            .as_array()
                            .is_some_and(|items| items.iter().all(scalar))
                }
            };
            ensure!(
                valid,
                "Comparison does not match the type of {}",
                rule.field
            );
        }
        ensure!(
            config
                .include
                .iter()
                .all(|field| fields.contains_key(field)),
            "One or more output fields are unavailable"
        );
        Ok(())
    }),
    schema: || {
        json!({
            "type":"object","required":["all","include","message"],"additionalProperties":false,
            "properties":{
                "all":{"type":"array","minItems":1,"maxItems":8,"items":{
                    "type":"object","required":["field","op","value"],"additionalProperties":false,
                    "properties":{"field":{"type":"string"},"op":{"enum":["eq","in","contains","lt","gt"]},"value":{"description":"Literal comparison value in the source field format. Arrays for in; a scalar for contains."}}
                }},
                "include":{"type":"array","maxItems":8,"items":{"type":"string"}},
                "message":{"type":"string","description":"Literal introduction. The selected include fields are added separately; do not use placeholders."},
                "snapshot":{"type":"object","description":"Use with a timer to check the latest signed record from an enabled event source.","required":["source","eventType","maxAgeSeconds"],"additionalProperties":false,"properties":{"source":{"type":"string"},"eventType":{"type":"string"},"maxAgeSeconds":{"type":"integer","minimum":60,"maximum":86400}}},
                "source":{"type":"object","description":"Fetch a structured record from an approved read-only tool before checking conditions.","required":["tool","args"],"additionalProperties":false,"properties":{"tool":{"type":"string"},"args":{"type":"object"}}}
            }
        })
    },
    describe: |value| {
        let config = config(value)?;
        let mut details = vec![config.message];
        if let Some(source) = config.snapshot {
            details.push(format!(
                "Latest {} / {} within {} seconds",
                source.source, source.event_type, source.max_age_seconds
            ));
        }
        if let Some(source) = config.source {
            details.push(format!("Read {} {}", source.tool, source.args));
        }
        for filter in config.all {
            let operation = match filter.op {
                Operator::Eq => "equals",
                Operator::In => "is one of",
                Operator::Contains => "contains",
                Operator::Lt => "less than",
                Operator::Gt => "greater than",
            };
            details.push(format!("{} {operation} {}", filter.field, filter.value));
        }
        if !config.include.is_empty() {
            details.push(format!("Show: {}", config.include.join(", ")));
        }
        Ok(details)
    },
    preflight: |server, value| {
        if let Some(source) = config(value)?.source {
            super::fetch::validate(server, &source)?;
        }
        Ok(())
    },
    run: |input, value| {
        Box::pin(async move {
            let config = config(value)?;
            let fetched = config.source.is_some();
            let (record, reference) = if let Some(source) = config.source {
                let record = super::fetch::read(input.server, input.wake_key, &source).await?;
                (
                    serde_json::to_string(&record)?,
                    format!("{} {}", source.tool, source.args),
                )
            } else if let Some(source) = config.snapshot {
                let (record, event_id) = events::latest(
                    input.server,
                    &source.source,
                    &source.event_type,
                    source.max_age_seconds,
                )
                .await?
                .context("No record within the reviewed freshness limit")?;
                let reference = format!("{}/{}", source.source, source.event_type);
                (
                    record,
                    event_id.map_or_else(|| reference.clone(), |id| format!("{reference}/{id}")),
                )
            } else {
                (
                    input
                        .event
                        .context("No source record was attached to the occurrence")?
                        .to_owned(),
                    input.wake_key.to_owned(),
                )
            };
            let record: Value = serde_json::from_str(&record)?;
            let checks = config
                .all
                .iter()
                .map(|filter| verdict(&record, filter))
                .collect::<Result<Vec<_>>>()?;
            if checks.contains(&false) {
                return Ok(Output {
                    body: None,
                    tools_used: i32::from(fetched),
                });
            }
            let mut body = config.message;
            for field in config.include {
                if let Some(value) = record.get(&field) {
                    let text = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned);
                    body.push_str(&format!(
                        "\n{}: {}",
                        field,
                        super::digest::plain(&text, 512)
                    ));
                }
            }
            if !reference.is_empty() {
                body.push_str(&format!("\nSource: {reference}"));
            }
            Ok(Output {
                body: Some(body),
                tools_used: i32::from(fetched),
            })
        })
    },
};
