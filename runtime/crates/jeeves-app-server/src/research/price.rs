//! Deterministic review of fixed prices in an owner's named public merchant feed.

use std::fmt;

use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use jeeves_loop::evidence::{Claim, Freshness, Review, adjudicate};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use super::{Observation, literal};

const PREFIX: &str = "price ";
const MAX_ROWS: usize = 128;
const TOTAL: &str = "checkout_total";

#[derive(Deserialize)]
struct Feed {
    offers: Vec<Offer>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Offer {
    store: Option<String>,
    sku: Option<String>,
    variant: Option<String>,
    date: Option<String>,
    conditions: Option<String>,
    amount: Option<String>,
    currency: Option<String>,
    price_kind: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Query {
    host: String,
    store: String,
    sku: String,
    variant: String,
    date: NaiveDate,
    conditions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

impl Query {
    pub(super) fn parse(line: &str) -> Option<Result<Self>> {
        let args = line.strip_prefix(PREFIX)?.trim_start();
        if !args.starts_with('{') {
            return None;
        }
        Some((|| {
            let query: Self = serde_json::from_str(args).context(
                "Use price followed by a JSON object with host, store, sku, variant, date and conditions",
            )?;
            let url = Url::parse(&format!("https://{}", query.host))?;
            ensure!(
                url.host_str() == Some(query.host.as_str())
                    && !query.host.is_empty()
                    && url.as_str() == format!("https://{}/", query.host),
                "Use a bare canonical merchant host without a path, port or credentials"
            );
            if let Some(url) = &query.url {
                ensure!(
                    super::source_url(url).is_ok() && query.authorizes(url),
                    "The optional feed URL must be HTTPS on the exact merchant host"
                );
            }
            for field in [&query.store, &query.sku, &query.variant, &query.conditions] {
                ensure!(
                    !field.is_empty()
                        && field.trim() == field
                        && field.chars().count() <= 100
                        && !field.chars().any(char::is_control),
                    "Store, SKU, variant and conditions must each be nonempty plain text"
                );
            }
            Ok(query)
        })())
    }

    pub(super) fn title(&self) -> String {
        format!(
            "{} · {} · {} · {} · {}",
            literal(&self.store),
            literal(&self.sku),
            literal(&self.variant),
            self.date,
            literal(&self.conditions),
        )
    }

    fn authorizes(&self, source: &str) -> bool {
        Url::parse(source).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some(self.host.as_str())
                && url.port().is_none()
        })
    }

    pub(super) fn assess<'a>(
        &self,
        observations: impl Iterator<Item = &'a Observation>,
    ) -> Assessment<'a> {
        let mut candidates = Vec::new();
        let entity = (
            self.store.as_str(),
            self.sku.as_str(),
            self.variant.as_str(),
        );
        let mut gaps = Vec::new();
        let mut read_feed = false;
        for source in observations.filter(|source| {
            self.authorizes(&source.url)
                && source
                    .requested_url()
                    .is_some_and(|url| self.authorizes(url))
                && source.intact()
        }) {
            let Ok(Feed { offers }) = serde_json::from_str(&source.text) else {
                continue;
            };
            if offers.len() > MAX_ROWS {
                continue;
            }
            read_feed = true;
            for (index, row) in offers.iter().enumerate() {
                if row.store.as_deref() != Some(entity.0) || row.sku.as_deref() != Some(entity.1) {
                    continue;
                }
                let variant = row.variant.as_deref().filter(|value| !value.is_empty());
                if variant.is_some_and(|value| value != entity.2) {
                    continue;
                }
                let evidence = RowEvidence {
                    source,
                    number: index + 1,
                };
                let day = row.date.as_deref().and_then(|s| {
                    let day = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
                    (day.to_string() == s).then_some(day)
                });
                let conditions = row.conditions.as_deref();
                let amount = row.amount.as_deref().and_then(Amount::parse);
                let kind = row.price_kind.as_deref();
                let cny = row.currency.as_deref() == Some("CNY");
                // Other explicitly dated offers or other known conditions do not enter
                // this comparison. Missing date or conditions remain a visible gap.
                if day.is_some_and(|day| day != self.date)
                    || conditions.is_some_and(|text| text != self.conditions && !text.is_empty())
                {
                    continue;
                }
                if let (Some(amount), Some(day), Some(_)) = (amount, day, variant)
                    && conditions == Some(self.conditions.as_str())
                    && kind == Some(TOTAL)
                    && cny
                {
                    candidates.push((
                        Claim {
                            entity,
                            recorded_at: Some(day),
                            identity: (),
                            value: amount,
                        },
                        evidence,
                    ));
                    continue;
                }
                if gaps.len() < 3 {
                    let mut missing = Vec::new();
                    if variant.is_none() {
                        missing.push("缺少商品规格");
                    }
                    if day.is_none() {
                        missing.push("缺少有效的适用日期");
                    }
                    if conditions != Some(self.conditions.as_str()) {
                        missing.push("缺少匹配的优惠条件");
                    }
                    if kind != Some(TOTAL) {
                        missing.push("未标明固定到手总价");
                    }
                    if !cny {
                        missing.push("缺少 CNY 币种");
                    }
                    if amount.is_none() {
                        missing.push("缺少有效金额");
                    }
                    let reason = missing.join("、");
                    let excerpt = match amount.filter(|_| cny) {
                        Some(amount) => format!("页面列出 {amount}，{reason}"),
                        None => reason,
                    };
                    gaps.push((evidence, excerpt));
                }
            }
        }
        let result = adjudicate(
            candidates,
            &entity,
            Freshness::Within(self.date..=self.date),
            |_| true,
        );
        match result {
            Review::Insufficient => Assessment::Unresolved { read_feed, gaps },
            Review::Confirmed((claim, source)) => Assessment::Confirmed(claim.value, source),
            Review::Conflict(rows) => Assessment::Conflict(
                rows.into_iter()
                    .map(|(claim, source)| (claim.value, source))
                    .collect(),
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Amount(u64);

impl Amount {
    fn parse(value: &str) -> Option<Self> {
        let (whole, fraction) = match value.split_once('.') {
            Some((whole, fraction)) if (1..=2).contains(&fraction.len()) => (whole, fraction),
            Some(_) => return None,
            None => (value, ""),
        };
        if !(1..=9).contains(&whole.len())
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let mut cents = fraction.parse::<u64>().unwrap_or(0);
        if fraction.len() == 1 {
            cents *= 10;
        }
        Some(Self(whole.parse::<u64>().ok()? * 100 + cents))
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "CNY {}.{:02}", self.0 / 100, self.0 % 100)
    }
}

pub(super) struct RowEvidence<'a> {
    source: &'a Observation,
    number: usize,
}

impl RowEvidence<'_> {
    fn citation(&self) -> String {
        format!(
            "<{}> · 报价记录 {} · 读取于 {}",
            self.source.url, self.number, self.source.fetched_at
        )
    }
}

pub(super) enum Assessment<'a> {
    Confirmed(Amount, RowEvidence<'a>),
    Conflict(Vec<(Amount, RowEvidence<'a>)>),
    Unresolved {
        read_feed: bool,
        gaps: Vec<(RowEvidence<'a>, String)>,
    },
}

impl Assessment<'_> {
    pub(super) fn state(&self) -> &'static str {
        match self {
            Self::Confirmed(..) => "price-supported",
            Self::Conflict(_) => "conflicting",
            Self::Unresolved { .. } => "unresolved",
        }
    }

    pub(super) fn supported(&self) -> bool {
        matches!(self, Self::Confirmed(..))
    }

    pub(super) fn render(&self) -> Vec<String> {
        let cite = |amount: &Amount, evidence: &RowEvidence<'_>| {
            format!(
                "商家列出的固定到手总价为 {amount}。\n\n{}",
                evidence.citation()
            )
        };
        match self {
            Self::Confirmed(amount, source) => vec![cite(amount, source)],
            Self::Conflict(rows) => {
                let mut lines = vec!["待核实：同一口径出现不同报价。".into()];
                lines.extend(
                    rows.iter()
                        .take(4)
                        .map(|(amount, source)| cite(amount, source)),
                );
                if rows.len() > 4 {
                    lines.push(format!("另外还有 {} 个不同报价。", rows.len() - 4));
                }
                lines
            }
            Self::Unresolved { read_feed, gaps } => {
                let mut lines = vec![if *read_feed {
                    "待核实：未找到符合门店、规格、日期和优惠条件的固定到手总价。".into()
                } else {
                    "待核实：没有读到所指定商家可用于核对的报价数据。".into()
                }];
                lines.extend(
                    gaps.iter()
                        .map(|(source, gap)| format!("{gap}。\n\n{}", source.citation())),
                );
                lines
            }
        }
    }
}
