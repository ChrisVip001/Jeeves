//! A bounded price watch repeats only a successful, read-only quote from the owner's comparison.
use crate::Server;
use anyhow::{anyhow, ensure, Context, Result};
use jeeves_protocol::{AcceptanceView, BudgetView, InteractionStartParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Number};
use sqlx::SqliteConnection;

const HOUR: i64 = 3_600_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    url: String,
    model: String,
    spec: String,
    below: Number,
    days: i32,
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct Product {
    url: String,
    model: String,
    spec: String,
    currency: String,
}

#[derive(Deserialize)]
struct Quote {
    #[serde(flatten)]
    product: Product,
    price: Number,
    shipping: Option<Number>,
}

fn minor(n: &Number) -> Result<i64> {
    let text = n.to_string();
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    ensure!(
        whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.len() <= 2
            && fraction.bytes().all(|b| b.is_ascii_digit()),
        "Expected a nonnegative monetary amount with at most two decimal places"
    );
    format!("{whole}{fraction:0<2}")
        .parse()
        .context("Amount out of range")
}

fn threshold(n: &Number) -> Result<i64> {
    let result = minor(n)?;
    ensure!(result > 0, "Price threshold must be positive");
    Ok(result)
}

pub(crate) struct Prepared {
    operation: String,
    product: Product,
    threshold: i64,
    hours: i32,
    start: i64,
}

impl Prepared {
    pub(super) fn budget(&self) -> BudgetView {
        BudgetView {
            tools: Some(self.hours),
            ..Default::default()
        }
    }
    pub(super) fn acceptance(&self) -> AcceptanceView {
        AcceptanceView {
            kind: "receiptConfirmed".into(),
            receipt_kind: Some("price_quote".into()),
            ref_prefix: Some(self.product.url.clone()),
            ..Default::default()
        }
    }
    pub(super) fn description(&self) -> String {
        format!(
            "{} {} · 每小时核对含运费到手价，低于 {} {} 首次通知，跟进 {} 天，只监测价格。",
            self.product.model,
            self.product.spec,
            amount(self.threshold),
            self.product.currency,
            self.hours / 24
        )
    }
    pub(super) async fn insert(&self, conn: &mut SqliteConnection, id: &str) -> Result<()> {
        sqlx::query("INSERT INTO price_watches(task_id,operation_id,quote_json,threshold_minor,expires_ms,next_check_ms) VALUES(?,?,?,?,?,?)")
            .bind(id).bind(&self.operation).bind(serde_json::to_string(&self.product)?).bind(self.threshold).bind(self.start+i64::from(self.hours)*HOUR).bind(self.start).execute(&mut *conn).await?;
        sqlx::query("UPDATE agent_tasks SET status='running' WHERE id=?")
            .bind(id)
            .execute(conn)
            .await?;
        Ok(())
    }
}

fn amount(n: i64) -> String {
    format!("{}.{:02}", n / 100, n % 100)
}

pub(super) async fn prepare(
    server: &Server,
    p: &InteractionStartParams,
    request: &Request,
) -> Result<Prepared> {
    ensure!(
        p.source.is_none(),
        "Private comparison sources are unavailable in channel conversations"
    );
    let hours = request
        .days
        .checked_mul(24)
        .filter(|h| *h > 0)
        .context("Invalid watch duration")?;
    let below = threshold(&request.below)?;
    let url = reqwest::Url::parse(&request.url)?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.host().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "Invalid quote source URL"
    );
    let rows: Vec<(String, String, String, String)> = sqlx::query_as("SELECT o.id,o.tool,o.output_digest,json_extract(l.payload,'$.content')
        FROM tool_operations o JOIN ledger l ON l.session_id=o.session_id AND l.turn_id=o.turn_id AND l.kind='tool_result'
        AND json_extract(l.payload,'$.toolUseId')=o.tool_use_id
        WHERE o.session_id=? AND o.task_id IS NULL AND o.status='succeeded' AND o.output_preview IS NOT NULL AND o.turn_id NOT LIKE ?
        AND CASE WHEN json_valid(json_extract(l.payload,'$.content')) THEN json_extract(json_extract(l.payload,'$.content'),'$.url') END=?
        ORDER BY l.seq DESC")
        .bind(&p.session_id).bind(format!("{}%",jeeves_protocol::CHANNEL_TURN_PREFIX)).bind(&request.url).fetch_all(&server.pool).await?;
    let tools = current_tools(server).await;
    for (operation, tool, digest, body) in rows {
        if !tools.effect(&tool).is_read()
            || jeeves_state::ledger::digest_of(body.as_bytes()) != digest
        {
            continue;
        }
        let Ok(quote) = serde_json::from_str::<Quote>(&body) else {
            continue;
        };
        if quote.product.model != request.model || quote.product.spec != request.spec {
            continue;
        }
        ensure!(
            quote.product.currency.len() == 3
                && quote
                    .product
                    .currency
                    .bytes()
                    .all(|c| c.is_ascii_uppercase()),
            "Quote must specify its currency"
        );
        return Ok(Prepared {
            operation,
            product: quote.product,
            threshold: below,
            hours,
            start: chrono::Utc::now().timestamp_millis(),
        });
    }
    Err(anyhow!("No matching settled read-only quote with source URL, model, spec, price and currency in this conversation"))
}

async fn current_tools(server: &Server) -> jeeves_tools::ToolRegistry {
    let settings = server.settings();
    server.mcp.refresh(&settings.home).await;
    crate::enabled_tools(
        &crate::connector_settings::apply(&settings.home, &server.mcp.apply(&server.tools)),
        &settings,
    )
}

pub(super) async fn exists(server: &Server, id: &str) -> Result<bool> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM price_watches WHERE task_id=?)")
            .bind(id)
            .fetch_one(&server.pool)
            .await?,
    )
}

pub(super) async fn amend(
    server: &Server,
    p: &InteractionStartParams,
    id: &str,
    below: &Number,
    invocation: Option<&str>,
) -> Result<String> {
    let value = threshold(below)?;
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    let revision: i64 = sqlx::query_scalar("UPDATE price_watches SET threshold_minor=?,revision=revision+1 WHERE task_id=? AND expires_ms>? AND EXISTS(SELECT 1 FROM agent_tasks WHERE id=? AND status IN ('pending','running','suspended')) RETURNING revision")
        .bind(value).bind(id).bind(chrono::Utc::now().timestamp_millis()).bind(id).fetch_optional(&mut *tx).await?.context("Watch has ended or is unavailable")?;
    let operation = super::handoff::operation_id(&mut tx, p, invocation).await?;
    sqlx::query("INSERT INTO task_amendments(task_id,revision,message_id,instruction,threshold_minor,operation_id) VALUES(?,?,?,?,?,?)")
        .bind(id).bind(revision).bind(&p.message_id).bind(&p.message).bind(value).bind(operation).execute(&mut *tx).await?;
    jeeves_state::Events::append_on(&mut tx, id, None, "amendment", Some(&p.message)).await?;
    tx.commit().await?;
    Ok(format!(
        "{} · 已将含运费到手价阈值更新为 {}。",
        id,
        amount(value)
    ))
}

#[derive(sqlx::FromRow)]
struct Due {
    task_id: String,
    session_id: String,
    tool: String,
    input_json: String,
    quote_json: String,
    threshold_minor: i64,
    revision: i64,
    expires_ms: i64,
    next_check_ms: i64,
}

pub(crate) async fn tick(server: &Server, now: i64) -> Result<()> {
    let due: Vec<Due> = sqlx::query_as("SELECT w.*,o.tool,o.input_json,t.session_id FROM price_watches w JOIN agent_tasks t ON t.id=w.task_id JOIN tool_operations o ON o.id=w.operation_id WHERE t.status IN ('pending','running') AND (w.next_check_ms<=? OR w.expires_ms<=?) ORDER BY w.next_check_ms")
        .bind(now).bind(now).fetch_all(&server.pool).await?;
    if due.is_empty() {
        return Ok(());
    }
    let tools = current_tools(server).await;
    for w in due {
        if let Err(error) = poll(server, &tools, &w, now).await {
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

impl Quote {
    fn delivered(&self, expected: &Product) -> Result<(i64, serde_json::Value)> {
        ensure!(
            &self.product == expected,
            "Source, model, specification or currency changed"
        );
        let price = minor(&self.price)?;
        let shipping = minor(self.shipping.as_ref().context("Shipping is unconfirmed")?)?;
        let total = price
            .checked_add(shipping)
            .context("Delivered price out of range")?;
        Ok((
            total,
            json!({"product":self.product,"priceMinor":price,"shippingMinor":shipping,"totalMinor":total}),
        ))
    }
}

async fn poll(
    server: &Server,
    tools: &jeeves_tools::ToolRegistry,
    w: &Due,
    now: i64,
) -> Result<()> {
    let mut tx = server.pool.begin_with("BEGIN IMMEDIATE").await?;
    if now >= w.expires_ms {
        sqlx::query("UPDATE agent_tasks SET status='failed',finished_at=datetime('now'),last_stop_reason='监测期限已到，没有确认低于阈值的到手价' WHERE id=? AND status IN ('pending','running')")
            .bind(&w.task_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(());
    }
    let claimed = sqlx::query("UPDATE price_watches SET next_check_ms=? WHERE task_id=? AND revision=? AND next_check_ms=? AND EXISTS(SELECT 1 FROM agent_tasks WHERE id=? AND status IN ('pending','running') AND spent_tools < json_extract(budget_json,'$.tools'))")
        .bind(now+HOUR).bind(&w.task_id).bind(w.revision).bind(w.next_check_ms).bind(&w.task_id).execute(&mut *tx).await?.rows_affected();
    if claimed == 0 {
        return Ok(());
    }
    sqlx::query("UPDATE agent_tasks SET spent_tools=spent_tools+1 WHERE id=?")
        .bind(&w.task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    let observed = read_quote(server, tools, w).await;
    let (total, evidence) = match observed {
        Ok((total, evidence)) => (Some(total), evidence),
        Err(e) => (
            None,
            json!({"unconfirmed":e.to_string().chars().take(512).collect::<String>()}),
        ),
    };
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
    if let Some(total) = total.filter(|n| *n < w.threshold_minor) {
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
        let changed = sqlx::query("UPDATE agent_tasks SET status='done',last_stop_reason=?,checks_json=?,finished_at=datetime('now') WHERE id=? AND status IN ('pending','running') AND EXISTS(SELECT 1 FROM price_watches WHERE task_id=? AND revision=? AND expires_ms>?)")
            .bind(&detail).bind(checks.to_string()).bind(&w.task_id).bind(&w.task_id).bind(w.revision).bind(chrono::Utc::now().timestamp_millis().max(now)).execute(&mut *tx).await?.rows_affected();
        if changed == 1 {
            sqlx::query("INSERT INTO receipts(task_id,kind,ref,status) VALUES(?,'price_quote',?,'confirmed')").bind(&w.task_id).bind(&product.url).execute(&mut *tx).await?;
            eprintln!(
                "(jeeves) price watch {} matched revision {}",
                w.task_id, w.revision
            );
        }
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn summary(server: &Server, id: &str) -> Result<String> {
    let row: (String, i64, i64, i64, String) = sqlx::query_as("SELECT w.quote_json,w.threshold_minor,w.expires_ms,w.revision,t.status FROM price_watches w JOIN agent_tasks t ON t.id=w.task_id WHERE w.task_id=?")
        .bind(id).fetch_one(&server.pool).await?;
    let product: Product = serde_json::from_str(&row.0)?;
    let expires = chrono::DateTime::from_timestamp_millis(row.2).context("Invalid expiry")?;
    Ok(format!(
        "{} {} · 含运费到手价低于 {} {} 时通知 · {} · 有效至 {} · v{} · {}",
        product.model,
        product.spec,
        amount(row.1),
        product.currency,
        product.url,
        expires,
        row.3,
        row.4
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmed_delivered_price_requires_the_same_product_and_known_shipping() {
        let raw = json!({"url":"https://shop.test/b","model":"B","spec":"256 GB","currency":"CNY","price":1679.99,"shipping":20});
        let expected: Product = serde_json::from_value(raw.clone()).unwrap();
        let quote: Quote = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(quote.delivered(&expected).unwrap().0, 169999);
        for (field, change) in [
            ("url", json!("https://shop.test/c")),
            ("model", json!("C")),
            ("spec", json!("128 GB")),
            ("currency", json!("USD")),
            ("shipping", json!(null)),
            ("price", json!(-1)),
            ("price", json!(1.002)),
        ] {
            let mut changed = raw.clone();
            changed[field] = change;
            let quote: Quote = serde_json::from_value(changed).unwrap();
            assert!(quote.delivered(&expected).is_err(), "{field}");
        }
    }
}
