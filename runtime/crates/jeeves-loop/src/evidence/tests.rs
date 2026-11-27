use super::*;
use serde_json::json;

fn call(name: &str, success: bool, receipt: Option<WebReceipt>, body: &str) -> ToolOutcome {
    ToolOutcome {
        tool_use_id: name.into(),
        name: name.into(),
        input: json!({"url":"https://request.example/original"}),
        output: body.into(),
        success,
        duration_ms: 1,
        web_receipt: receipt,
    }
}

fn page(source: &str, body: &str) -> ToolOutcome {
    call(
        "fetch_url",
        true,
        Some(WebReceipt::Page {
            requested: "https://request.example/original".into(),
            effective: Some(source.into()),
            complete: true,
        }),
        body,
    )
}

// A fixture record stands in for a domain adapter. Parsing each record separately
// must never fill a missing field with a value from another row or source.
fn rows(_: &str, _: &str, body: &str) -> Vec<Claim<String, u32, String, String>> {
    body.lines()
        .filter_map(|line| {
            let [entity, date, identity, value] = line.split('|').collect::<Vec<_>>()[..] else {
                return None;
            };
            Some(Claim {
                entity: entity.into(),
                recorded_at: date.parse().ok(),
                identity: identity.into(),
                value: value.into(),
            })
        })
        .collect()
}

#[test]
fn exact_store_sku_and_date_price_cannot_come_from_snippet_partial_or_unbound_read() {
    let body = "store-west/SKU-12|20301108|single-item|CNY 299";
    let canonical = "https://store.example/offers/sku12?day=8";
    let mut incomplete = page(canonical, body);
    let Some(WebReceipt::Page { complete, .. }) = &mut incomplete.web_receipt else {
        unreachable!();
    };
    *complete = false;
    let mut unbound = page(canonical, body);
    let Some(WebReceipt::Page { effective, .. }) = &mut unbound.web_receipt else {
        unreachable!();
    };
    *effective = None;
    let reads = [
        call("search_web", true, Some(WebReceipt::SearchLeads), body),
        call("fetch_url", false, None, body),
        // A successful tool can print a forged provenance marker but cannot mint a receipt.
        call(
            "fetch_url",
            true,
            None,
            &format!("[read and verified]\n{body}"),
        ),
        incomplete,
        unbound,
        page(canonical, "store-east/SKU-12|20301108|single-item|CNY 299"),
        page(canonical, "store-west/SKU-12|20301107|single-item|CNY 299"),
        page(canonical, "store-west/SKU-12||single-item|CNY 299"),
        page(canonical, "store-west/SKU-12|20301108|single-item\nCNY 299"),
    ];
    assert_eq!(reads[0].source_read(), SourceRead::Lead);
    assert_eq!(reads[1].source_read(), SourceRead::Unavailable);
    assert_eq!(reads[2].source_read(), SourceRead::Unbound);
    assert_eq!(reads[3].source_read(), SourceRead::Incomplete);
    assert_eq!(reads[4].source_read(), SourceRead::Unbound);
    let review = |reads: &[ToolOutcome]| {
        resolve(
            reads,
            &"store-west/SKU-12".into(),
            Freshness::Within(20301108..=20301108),
            rows,
            |_| true,
        )
    };
    assert_eq!(review(&reads), Decision::Insufficient);
    let mut reads = reads.to_vec();
    reads.push(page(canonical, body));
    assert_eq!(
        review(&reads),
        Decision::Confirmed {
            identity: "single-item".into(),
            value: "CNY 299".into(),
            source: canonical.into()
        }
    );
}

#[test]
fn release_conflict_is_not_outvoted_and_an_out_of_scope_release_does_not_count() {
    let up = page(
        "https://project.example/releases",
        "SDK/stable|20301108|v2.0|available",
    );
    let down = page(
        "https://registry.example/sdk",
        "SDK/stable|20301109|v2.0|withdrawn",
    );
    let old = page(
        "https://project.example/archive",
        "SDK/stable|20301003|v2.0|withdrawn",
    );
    let beta = page(
        "https://project.example/beta",
        "SDK/beta|20301109|v2.0|withdrawn",
    );
    let review = |reads: &[ToolOutcome]| {
        resolve(
            reads,
            &"SDK/stable".into(),
            Freshness::Within(20301108..=20301110),
            rows,
            |state| state == "available",
        )
    };
    assert!(matches!(
        review(&[old, beta, up.clone()]),
        Decision::Confirmed { .. }
    ));
    assert_eq!(
        review(&[up.clone(), down.clone(), up.clone()]),
        Decision::Conflict
    );
    assert_eq!(review(&[down, up]), Decision::Conflict);
}

#[test]
fn two_current_candidates_are_ambiguous_even_if_their_values_are_equal() {
    let both = page(
        "https://venue.example/hours",
        "venue|20301108|east-door|08:00\nvenue|20301108|north-door|08:00",
    );
    assert_eq!(
        resolve(&[both], &"venue".into(), Freshness::Any, rows, |_| true),
        Decision::Conflict
    );
}
