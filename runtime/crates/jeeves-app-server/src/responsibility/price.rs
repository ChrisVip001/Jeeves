//! Admit the next saved quote observation and settle it with its source and chat receipt.
use super::action::{Adapter, Output, Settlement};
use crate::{
    interaction::price_watch::{amount, current_tools, Product, Quote, HOUR},
    Server,
};
use anyhow::{ensure, Result};
use jeeves_state::responsibility_runs as runs;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(sqlx::FromRow, Serialize, Deserialize)]
struct Due {
    task_id: String,
    duty_id: String,
    duty_revision: i32,
    session_id: String,
    tool: String,
    input_json: String,
    quote_json: String,
    threshold_minor: i64,
    revision: i64,
    expires_ms: i64,
    next_check_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct Observation {
    total: Option<i64>,
    evidence: Value,
}

pub(super) const ADAPTER: Adapter = Adapter {
    kind: "productQuote",
    label: "Verify the current delivered price",
    uses_model: false,
    internal: true,
    schema: || json!({"type":"object","additionalProperties":false}),
    describe: |value| {
        ensure!(
            value.as_object().is_some_and(serde_json::Map::is_empty),
            "Unexpected quote configuration"
        );
        Ok(vec![])
    },
    preflight: |_, _| Ok(()),
    run: |input, _| {
        Box::pin(async move {
            let watch: Due = serde_json::from_str(
                input
                    .event
                    .ok_or_else(|| anyhow::anyhow!("Missing assigned quote"))?,
            )?;
            let tools = current_tools(input.server).await;
            let observed = read_quote(input.server, &tools, &watch).await;
            let observation = match observed {
                Ok((total, evidence)) => Observation {
                    total: Some(total),
                    evidence,
                },
                Err(error) => Observation {
                    total: None,
                    evidence: json!({"unconfirmed":error.to_string().chars().take(512).collect::<String>()}),
                },
            };
            Ok(Output {
                body: Some(serde_json::to_string(&observation)?),
                tools_used: 1,
            })
        })
    },
    settle: Some(|context, _| Box::pin(settle(context))),
};

pub(crate) async fn tick(server: &Server, now: i64) -> Result<()> {
    let due: Vec<Due> = sqlx::query_as("SELECT w.*,o.tool,o.input_json,t.session_id,r.id AS duty_id,r.revision AS duty_revision FROM price_watches w JOIN responsibilities r ON r.id='watch-'||w.task_id AND r.clock_owner='price-watch' JOIN agent_tasks t ON t.id=w.task_id JOIN tool_operations o ON o.id=w.operation_id WHERE (r.status='active' OR w.expires_ms<=?) AND t.status IN ('pending','running') AND (w.next_check_ms<=? OR w.expires_ms<=?) ORDER BY w.next_check_ms")
        .bind(now).bind(now).bind(now).fetch_all(&server.pool).await?;
    if due.is_empty() {
        return Ok(());
    }
    for w in due {
        if let Err(error) = poll(server, &w, now).await {
            eprintln!("(jeeves) price watch {}: {error}", w.task_id);
        }
    }
    Ok(())
}

async fn read_quote(
    server: &Server,
    tools: &jeeves_tools::ToolRegistry,
    watch: &Due,
) -> Result<(i64, serde_json::Value)> {
    ensure!(
        tools.effect(&watch.tool).is_read(),
        "Quoted source is disabled or no longer read-only"
    );
    let ctx = server.tool_ctx(&watch.session_id, None, None);
    let result = tools
        .execute_result(ctx, &watch.tool, serde_json::from_str(&watch.input_json)?)
        .await;
    ensure!(result.success, "Quote lookup failed: {}", result.text);
    let quote: Quote = serde_json::from_str(&result.text)?;
    quote.delivered(&serde_json::from_str(&watch.quote_json)?)
}

async fn poll(server: &Server, w: &Due, now: i64) -> Result<()> {
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    if now >= w.expires_ms {
        sqlx::query("UPDATE agent_tasks SET status='failed',finished_at=datetime('now'),last_stop_reason='监测期限已到，没有确认低于阈值的到手价' WHERE id=? AND status IN ('pending','running')")
            .bind(&w.task_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(());
    }
    let claimed = sqlx::query("UPDATE price_watches SET next_check_ms=? WHERE task_id=? AND revision=? AND next_check_ms=? AND EXISTS(SELECT 1 FROM responsibilities WHERE id=? AND revision=? AND status='active')
        AND NOT EXISTS(SELECT 1 FROM responsibility_runs WHERE responsibility_id=? AND status='preparing')
        AND EXISTS(SELECT 1 FROM agent_tasks WHERE id=? AND status IN ('pending','running') AND spent_tools < json_extract(budget_json,'$.tools'))")
        .bind(now+HOUR).bind(&w.task_id).bind(w.revision).bind(w.next_check_ms).bind(&w.duty_id).bind(w.duty_revision).bind(&w.duty_id).bind(&w.task_id).execute(&mut *tx).await?.rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    let key = format!("price-watch:{}:{}", w.task_id, w.next_check_ms);
    runs::enqueue_bound(
        &mut tx,
        &w.duty_id,
        &key,
        w.next_check_ms,
        Some(&serde_json::to_string(w)?),
    )
    .await?;
    sqlx::query("UPDATE agent_tasks SET spent_tools=spent_tools+1 WHERE id=?")
        .bind(&w.task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let (sink, _rx) = crate::EventSink::channel();
    if let Some(work) = runs::take_key(
        &server.pool,
        chrono::Utc::now().timestamp_millis(),
        Some(&key),
    )
    .await?
    {
        super::worker::process(server, &sink, &work).await?;
    }
    Ok(())
}

async fn settle(context: Settlement<'_>) -> Result<(&'static str, String)> {
    let server = context.server;
    let key = &context.work.wake_key;
    let w: Due = serde_json::from_str(
        context
            .work
            .event_json
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing assigned quote"))?,
    )?;
    let Observation { total, evidence } = serde_json::from_str(
        context
            .output
            .body
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing quote observation"))?,
    )?;
    let now = chrono::Utc::now().timestamp_millis();
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    sqlx::query("INSERT INTO watch_observations(task_id,checked_ms,revision,threshold_minor,evidence) VALUES(?,?,?,?,?)")
        .bind(&w.task_id).bind(now).bind(w.revision).bind(w.threshold_minor).bind(evidence.to_string()).execute(&mut *tx).await?;
    let measurement = match total {
        Some(n) => format!(
            "v{} · 含运费到手价 {}，阈值 {}",
            w.revision,
            amount(n),
            amount(w.threshold_minor)
        ),
        None => format!(
            "v{} · {}",
            w.revision,
            evidence["unconfirmed"].as_str().unwrap_or_default()
        ),
    };
    jeeves_state::Events::append_on(
        &mut tx,
        &w.task_id,
        None,
        "price_observation",
        Some(&measurement),
    )
    .await?;
    let mut delivery = None;
    if let Some(total) =
        total.filter(|n| super::records::less_than(&json!(n), &json!(w.threshold_minor)))
    {
        let product: Product = serde_json::from_str(&w.quote_json)?;
        let detail = format!(
            "{} {} · 含运费到手价 {} {}，低于 {}。{}",
            product.model,
            product.spec,
            amount(total),
            product.currency,
            amount(w.threshold_minor),
            product.url
        );
        let checks = json!([{"label":"含运费到手价低于最新阈值","passed":true,"detail":detail}]);
        let changed = sqlx::query("UPDATE agent_tasks SET status='done',last_stop_reason=?,checks_json=?,finished_at=datetime('now') WHERE id=? AND status IN ('pending','running') AND EXISTS(SELECT 1 FROM price_watches WHERE task_id=? AND revision=? AND expires_ms>?)
             AND EXISTS(SELECT 1 FROM responsibilities WHERE id=? AND status='active' AND revision=?)")
            .bind(&detail).bind(checks.to_string()).bind(&w.task_id).bind(&w.task_id).bind(w.revision).bind(chrono::Utc::now().timestamp_millis().max(now)).bind(&w.duty_id).bind(w.duty_revision).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("INSERT INTO receipts(task_id,kind,ref,status) VALUES(?,'price_quote',?,'confirmed')").bind(&w.task_id).bind(&product.url).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO chat_log(role,session_id,source,task_id,notice_key,content)
                SELECT 'assistant',session_id,'task_completion',task_id,CAST(id AS TEXT),reason FROM task_completions
                WHERE task_id=? AND status='done' AND session_id<>'jeeves-main' ORDER BY id DESC LIMIT 1")
                .bind(&w.task_id).execute(&mut *tx).await?;
            let notice:String=sqlx::query_scalar("SELECT c.content FROM chat_log c JOIN task_completions t ON CAST(t.id AS TEXT)=c.notice_key AND t.task_id=c.task_id WHERE c.task_id=? AND c.source='task_completion' AND t.status='done' ORDER BY c.id DESC LIMIT 1")
                .bind(&w.task_id).fetch_one(&mut *tx).await?;
            delivery = Some(notice);
        }
    }
    let detail = delivery.as_deref().unwrap_or(&measurement);
    sqlx::query("UPDATE responsibility_runs SET body=?,tools_used=1 WHERE wake_key=?")
        .bind(evidence.to_string())
        .bind(key)
        .execute(&mut *tx)
        .await?;
    runs::finish_on(
        &mut tx,
        key,
        if delivery.is_some() {
            "confirmed"
        } else {
            "draft"
        },
        detail,
        0,
    )
    .await?;
    tx.commit().await?;
    eprintln!("(jeeves) price watch {}: {}", w.task_id, detail);
    Ok((
        if delivery.is_some() {
            "confirmed"
        } else {
            "draft"
        },
        detail.to_owned(),
    ))
}
