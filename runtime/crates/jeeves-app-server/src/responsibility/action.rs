//! Registry of executable, versioned recurring actions and their review projection.
use super::{digest, trigger, Server};
use anyhow::{ensure, Context, Result};
use chrono::{DateTime, Utc};
use jeeves_protocol::{
    BudgetView, ResponsibilityAction, ResponsibilityDelivery, ResponsibilityDigest,
    ResponsibilityPlan, ResponsibilityPresentation,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, time::Duration};

type OutputFuture<'a> = Pin<Box<dyn Future<Output = Result<Output>> + Send + 'a>>;
type Describe = fn(&Value) -> Result<Vec<String>>;
type Check = fn(&Server, &Value) -> Result<()>;
type FieldMap = std::collections::BTreeMap<String, super::events::Field>;
type CheckSource = fn(&Server, &Value, Option<&FieldMap>) -> Result<()>;

pub(super) struct Input<'a> {
    pub server: &'a Server,
    pub at: DateTime<Utc>,
    pub wake_key: &'a str,
    pub event: Option<&'a str>,
    pub budget: &'a BudgetView,
}

pub(super) struct Output {
    pub body: Option<String>,
    pub tools_used: i32,
}

pub(super) struct Adapter {
    pub kind: &'static str,
    pub label: &'static str,
    pub uses_model: bool,
    pub initial_tools: i32,
    pub schema: fn() -> Value,
    pub describe: Describe,
    pub preflight: Check,
    pub check_source: Option<CheckSource>,
    pub run: for<'a> fn(Input<'a>, &'a Value) -> OutputFuture<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reminder {
    text: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DigestSources {
    Named(Vec<String>),
    Generic(Vec<super::digest::Source>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Digest {
    sources: DigestSources,
    options: ResponsibilityDigest,
}

fn decode<T: serde::de::DeserializeOwned>(value: &Value) -> Result<T> {
    Ok(serde_json::from_value(value.clone())?)
}

fn instruction(value: &Value) -> Result<String> {
    let Reminder { text } = decode(value)?;
    ensure!(
        !text.trim().is_empty() && text.len() <= 4096,
        "Supply a text of 1 to 4096 bytes"
    );
    Ok(text)
}

const REMINDER: Adapter = Adapter {
    kind: "reminder",
    label: "Reminder",
    uses_model: false,
    initial_tools: 0,
    check_source: None,
    schema: || json!({"type":"object","required":["text"],"properties":{"text":{"type":"string"}},"additionalProperties":false}),
    describe: |value| Ok(vec![instruction(value)?]),
    preflight: |_, _| Ok(()),
    run: |_, value| {
        Box::pin(async move {
            Ok(Output {
                body: Some(instruction(value)?),
                tools_used: 0,
            })
        })
    },
};

const DIGEST: Adapter = Adapter {
    kind: "digest",
    label: "Source digest",
    uses_model: false,
    initial_tools: 0,
    check_source: None,
    schema: || {
        json!({"type":"object","required":["sources","options"],"properties":{
        "sources":{"type":"array","minItems":1,"maxItems":8,"items":{"anyOf":[
          {"enum":["calendar","openTasks","unreadMail"]},
          {"type":"object","required":["label","tool","args"],"additionalProperties":false,"properties":{"label":{"type":"string"},"tool":{"type":"string"},"args":{"type":"object"}}}
        ]}},
        "options":{"type":"object","required":["timeZone","calendarWindow","layout"],"additionalProperties":false,
          "properties":{"timeZone":{"type":"string"},"calendarWindow":{"enum":["today","tomorrow","thisWeek","nextWeek"]},"layout":{"enum":["paragraphs","list"]}}}
    },"additionalProperties":false})
    },
    describe: |value| {
        let config: Digest = decode(value)?;
        let labels = match &config.sources {
            DigestSources::Named(s) => {
                digest::validate(s, &config.options)?;
                s.clone()
            }
            DigestSources::Generic(s) => {
                digest::validate_generic(s, &config.options)?;
                s.iter().map(|source| source.label.clone()).collect()
            }
        };
        Ok(vec![labels.join(", "), format!("{:?}", config.options.calendar_window)])
    },
    preflight: |server, value| {
        let config: Digest = decode(value)?;
        match &config.sources {
            DigestSources::Named(s) => digest::preflight(server, s),
            DigestSources::Generic(s) => digest::preflight_generic(server, s),
        }
    },
    run: |input, value| {
        Box::pin(async move {
            let config: Digest = decode(value)?;
            let (count, body) = match &config.sources {
                DigestSources::Named(sources) => {
                    ensure!(
                        input.budget.tools.is_none_or(|n| n as usize >= sources.len()),
                        "Tool budget does not cover the sources"
                    );
                    (sources.len(), digest::render(input.server, &config.options, sources, input.at).await?)
                }
                DigestSources::Generic(sources) => {
                    ensure!(
                        input.budget.tools.is_none_or(|n| n as usize >= sources.len()),
                        "Tool budget does not cover the sources"
                    );
                    (sources.len(), digest::render_generic(input.server, &config.options, sources, input.wake_key).await?)
                }
            };
            Ok(Output { body: Some(body), tools_used: count as i32 })
        })
    },
};

fn adapters() -> &'static [Adapter] {
    &[
        REMINDER,
        DIGEST,
        super::fetch::ADAPTER,
        super::model::ADAPTER,
        super::research::ADAPTER,
        super::records::ADAPTER,
    ]
}

pub(super) fn find(action: &ResponsibilityAction) -> Result<&'static Adapter> {
    adapters()
        .iter()
        .find(|adapter| adapter.kind == action.kind && action.version == 1)
        .with_context(|| {
            format!(
                "Unsupported action {} version {}",
                action.kind, action.version
            )
        })
}

pub(super) fn catalog() -> Value {
    Value::Array(
        adapters()
            .iter()
            .map(|adapter| {
                json!({
                    "kind":adapter.kind,"version":1,"label":adapter.label,
                    "usesModel":adapter.uses_model,"configSchema":(adapter.schema)()
                })
            })
            .collect(),
    )
}

pub(super) fn validate(plan: &ResponsibilityPlan, budget: &BudgetView) -> Result<()> {
    let adapter = find(&plan.action)?;
    ensure!(
        plan.action.config.to_string().len() <= 16_384,
        "Action configuration is too large"
    );
    (adapter.describe)(&plan.action.config)?;
    if adapter.uses_model {
        ensure!(
            budget.tokens.is_some_and(|n| (128..=16_384).contains(&n)),
            "Choose a model budget between 128 and 16384 tokens"
        );
    }
    crate::connector_settings::publish::resolve(&plan.delivery)?;
    Ok(())
}

pub(super) fn preflight(server: &Server, plan: &ResponsibilityPlan) -> Result<()> {
    let fields = super::events::preflight(server, &plan.trigger)?;
    let adapter = find(&plan.action)?;
    if let Some(check_source) = adapter.check_source {
        check_source(server, &plan.action.config, fields.as_ref())?;
    }
    (adapter.preflight)(server, &plan.action.config)
}

pub(super) fn presentation(plan: &ResponsibilityPlan) -> ResponsibilityPresentation {
    let (action, details, uses_model, blocked) = match find(&plan.action)
        .and_then(|adapter| Ok((adapter, (adapter.describe)(&plan.action.config)?)))
    {
        Ok((adapter, details)) => (adapter.label.into(), details, adapter.uses_model, None),
        Err(error) => (
            plan.action.kind.clone(),
            vec![],
            false,
            Some(error.to_string()),
        ),
    };
    let delivery = match crate::connector_settings::publish::resolve(&plan.delivery) {
        Ok(Some((adapter, target))) => format!("{} (approval required): {target}", adapter.label),
        Ok(None) if matches!(plan.delivery, ResponsibilityDelivery::Chat) => "Main chat".into(),
        Ok(None) => "Private draft".into(),
        Err(error) => error.to_string(),
    };
    ResponsibilityPresentation {
        trigger: trigger::describe(&plan.trigger),
        action,
        details,
        delivery,
        uses_model,
        blocked,
    }
}

pub(super) async fn sample(
    server: &Server,
    plan: &ResponsibilityPlan,
    at: DateTime<Utc>,
) -> Result<String> {
    let adapter = find(&plan.action)?;
    preflight(server, plan)?;
    let record = super::events::sample(server, &plan.trigger).await?;
    if adapter.uses_model
        || (record.is_none()
            && matches!(
                plan.trigger,
                jeeves_protocol::ResponsibilityTrigger::Event { .. }
            ))
    {
        return Ok(format!(
            "Preview · {} · {}",
            adapter.label,
            (adapter.describe)(&plan.action.config)?.join(" · ")
        ));
    }
    let budget = BudgetView {
        tools: Some(3),
        ..Default::default()
    };
    let preview = tokio::time::timeout(
        Duration::from_secs(20),
        (adapter.run)(
            Input {
                server,
                at,
                wake_key: "",
                event: record.as_deref(),
                budget: &budget,
            },
            &plan.action.config,
        ),
    )
    .await??;
    Ok(preview.body.unwrap_or_else(|| "No matching records".into()))
}

pub(super) fn legacy_digest(
    work: &jeeves_state::responsibility_runs::Work,
) -> Result<ResponsibilityPlan> {
    let options: ResponsibilityDigest = serde_json::from_str(
        work.digest_json
            .as_deref()
            .context("Responsibility has no executable action")?,
    )?;
    Ok(ResponsibilityPlan {
        trigger: jeeves_protocol::ResponsibilityTrigger::Cron {
            expression: String::new(),
            time_zone: options.time_zone.clone(),
        },
        action: ResponsibilityAction {
            kind: DIGEST.kind.into(),
            version: 1,
            config: json!({"sources":serde_json::from_str::<Value>(&work.sources_json)?,"options":options}),
        },
        delivery: ResponsibilityDelivery::Chat,
    })
}
