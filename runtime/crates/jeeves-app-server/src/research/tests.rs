use super::*;

fn source(state: &mut State, url: &str, text: &str) {
    state.observations.push(Observation {
        url: url.into(),
        text: text.into(),
        digest: jeeves_state::ledger::digest_of(text.as_bytes()),
        fetched_at: "2026-11-26T10:00:00Z".into(),
        receipt: Some(WebReceipt::Page {
            requested: url.into(),
            effective: Some(url.into()),
            complete: true,
        }),
    });
}

#[test]
fn one_document_can_support_distinct_questions_but_a_mirror_cannot_create_a_conflict() {
    let export = "March electricity exports were 24 GWh.";
    let import = "March electricity imports were 42 GWh.";
    let both = format!("{export} {import}");
    let mut state = State::new(
        vec!["March exports?".into(), "March imports?".into()],
        false,
    );
    source(&mut state, "https://example.test/official", &both);
    source(&mut state, "https://other.test/reprint", &both);
    state
        .record(json!({"action":"record","question":1,"citations":[{"source":1,"quote":export}]}))
        .unwrap();
    assert!(!state.render().1);
    assert!(
        state
            .record(json!({
                "action":"record","question":2,"conflicting":true,
                "citations":[{"source":1,"quote":import},{"source":2,"quote":import}]
            }))
            .is_err()
    );
    state
        .record(json!({"action":"record","question":2,"citations":[{"source":1,"quote":import}]}))
        .unwrap();
    assert!(state.render().1);
    let revised = "March electricity imports were 39 GWh.";
    source(&mut state, "https://example.test/revised", revised);
    state
        .record(json!({
            "action":"record","question":2,"conflicting":true,
            "citations":[{"source":1,"quote":import},{"source":3,"quote":revised}]
        }))
        .unwrap();
    assert!(!state.render().1);
    assert!(state.render().0.contains("conflicting excerpts"));
    assert!(
        state
            .record(json!({
                "action":"record","question":2,"citations":[{"source":3,"quote":revised}]
            }))
            .is_err()
    );
}

#[test]
fn user_scope_and_source_identity_are_bounded() {
    assert!(command("/web_research_not_a_command", QUICK).is_none());
    assert!(command(QUICK, QUICK).unwrap().is_err());
    assert_eq!(
        command("/web_research\n A?\n B?", QUICK)
            .unwrap()
            .unwrap()
            .questions,
        ["A?", "B?"]
    );
    assert!(source_url("https://user:password@example.test/").is_err());
    assert!(source_url("file:///tmp/example").is_err());
    assert_eq!(
        source_url("https://example.test/page?date=4#table").unwrap(),
        "https://example.test/page?date=4"
    );
}

#[test]
fn required_sources_survive_restore_and_need_a_bound_read_for_completion() {
    let request = command(
        "/web_research\nRead [the report](https://example.test/wiki/Report_(May)) and https://example.test/update。\nsource https://example.test/update#top",
        QUICK,
    )
    .unwrap()
    .unwrap();
    let report = "https://example.test/wiki/Report_(May)";
    let update = "https://example.test/update";
    assert_eq!(request.required_sources, [update, report]);
    assert_eq!(request.questions.len(), 1);

    let mut state = State::new(request.questions, true);
    state.required_sources = request.required_sources;
    let text = "Capacity was 68 percent.";
    source(&mut state, report, text);
    state
        .record(json!({"action":"record","question":1,"citations":[{"source":1,"quote":text}]}))
        .unwrap();
    source(&mut state, update, "Update.");
    state.observations[1].receipt = None;
    let saved = serde_json::to_string(&state).unwrap();
    let mut restored = serde_json::from_str::<State>(&saved).unwrap().restore();
    let (reply, complete) = restored.render();
    assert!(!complete && reply.contains(&format!("Required source unread: <{update}>")));
    source(&mut restored, update, "Update.");
    assert!(restored.render().1);
}

#[test]
fn explicitly_requested_verification_stays_open_when_the_model_records_an_excerpt() {
    let quote = "Capacity was 68 percent on May 4.";
    let mut state = State::new(
        vec![
            "Read https://example.test/report, then confirm the capacity on May 5.".into(),
            "请打开 https://example.test/report，确认 5 月 5 日的水位。".into(),
            "What capacity did the report give for May 4?".into(),
        ],
        false,
    );
    source(&mut state, "https://example.test/report", quote);
    for question in 1..=3 {
        state.record(json!({"action":"record","question":question,"citations":[{"source":1,"quote":quote}]})).unwrap();
    }
    let (report, complete) = state.render();
    assert!(!complete);
    assert_eq!(report.matches("Unresolved:").count(), 2);
    assert_eq!(report.matches("Background excerpts").count(), 2);
    assert_eq!(report.matches(quote).count(), 3);
    assert_eq!(state.status()["questions"][2]["state"], "excerpt-recorded");
}

#[test]
fn bounded_search_reserves_reads_without_blocking_a_refinement_after_irrelevant_leads() {
    let mut state = State::new(vec!["one".into(), "two".into(), "three".into()], false);
    assert!(state.charge_web(false).is_ok());
    state.leads = (1..=3)
        .map(|i| format!("https://example.test/{i}"))
        .collect();
    assert!(state.charge_web(false).is_err());
    assert_eq!(state.attempts, 1);
    state.dismissed.push(state.leads[0].clone());
    assert!(state.charge_web(false).is_ok());
    assert!(state.charge_web(false).is_err());
    assert!(state.charge_web(true).is_ok());
}

#[test]
fn bound_observation_preserves_final_url_and_rejects_unbound_or_partial_reads() {
    let original = "https://example.test/old?date=4";
    let effective = "https://example.test/report?date=4";
    let text = "Capacity was 68 percent on May 4.";
    for (url, complete) in [(None, true), (Some(effective), false)] {
        let capture = WebReadCapture::direct();
        capture.set(WebReceipt::Page {
            requested: original.into(),
            effective: url.map(str::to_owned),
            complete,
        });
        assert!(Observation::new(original, &capture, text).is_err());
    }
    let capture = WebReadCapture::direct();
    capture.set(WebReceipt::Page {
        requested: original.into(),
        effective: Some(effective.into()),
        complete: true,
    });
    assert!(Observation::new(effective, &capture, text).is_err());
    let mut state = State::new(vec!["Capacity on May 4?".into()], false);
    state
        .observations
        .push(Observation::new(original, &capture, text).unwrap());
    assert!(!state.eligible(original) && !state.eligible(effective));
    state
        .record(json!({"action":"record","question":1,"citations":[{"source":1,"quote":text}]}))
        .unwrap();
    let (reply, complete) = state.render();
    assert!(complete && reply.contains(effective) && !reply.contains(original));
}

#[test]
fn restoring_legacy_and_modified_observations_discards_their_bindings_and_preserves_source_ids() {
    let url = "https://example.test/record";
    let quote = "Capacity was 68 percent.";
    let mut state = State::new(vec!["Capacity?".into(), "Test?".into()], true);
    source(&mut state, url, quote);
    source(&mut state, "https://example.test/changed", quote);
    let record = |question: usize, id: usize| json!({"action":"record","question":question,"citations":[{"source":id,"quote":quote}]});
    state.record(record(1, 1)).unwrap();
    state.record(record(2, 2)).unwrap();
    let mut serialized = serde_json::to_value(state).unwrap();
    serialized["observations"][0]
        .as_object_mut()
        .unwrap()
        .remove("receipt");
    serialized["observations"][1]["text"] = json!(format!("Altered copy: {quote}"));
    let mut restored = serde_json::from_value::<State>(serialized)
        .unwrap()
        .restore();
    assert_eq!(restored.observation_count(), 0);
    assert_eq!(restored.status()["sources"], json!([]));
    assert!(restored.eligible(url));
    assert!(restored.record(record(1, 1)).is_err());
    assert!(!restored.render().1);
    assert!(!restored.render().0.contains(quote));
    source(&mut restored, url, quote);
    restored.record(record(1, 3)).unwrap();
    assert_eq!(restored.status()["sources"][0]["id"], 3);
}

#[test]
fn projection_cuts_view_without_changing_the_receipt_body_or_digest() {
    let url = "https://example.test/record";
    let mut state = State::new(vec!["Capacity?".into()], false);
    let body = format!("{}\nCapacity was 68 percent.", "一".repeat(MAX_DISPLAY));
    source(&mut state, url, &body);
    let saved = serde_json::to_string(&state).unwrap();
    let restored = serde_json::from_str::<State>(&saved).unwrap().restore();
    let observation = restored.observation(1).unwrap();
    assert_eq!(observation.display().chars().count(), MAX_DISPLAY);
    assert_eq!(observation.text, body);
    assert!(observation.matches_quote("Capacity was 68 percent."));
}

const PRICE_QUERY: &str = r#"price {"host":"shop.example","store":"南山店","sku":"A250","variant":"250ml","date":"2026-11-27","conditions":"无优惠"}"#;
const PRICE_SOURCE: &str = "https://shop.example/quotes";
const PRICE_UPDATE: &str = "https://shop.example/quotes/revision";

fn price_state() -> State {
    let request = command(&format!("{QUICK}\n{PRICE_QUERY}"), QUICK)
        .unwrap()
        .unwrap();
    let mut state = State::new(request.questions, false);
    state.price_queries = request.price_queries;
    state
}

fn price_row(amount: &str) -> Value {
    json!({
        "store":"南山店","sku":"A250","variant":"250ml","date":"2026-11-27",
        "conditions":"无优惠","currency":"CNY","priceKind":"checkout_total","amount":amount
    })
}

#[test]
fn closed_price_scope_refuses_wrong_fields_starting_price_and_fields_spliced_across_rows() {
    for (field, mismatch) in [
        ("store", "福田店"),
        ("sku", "B250"),
        ("variant", "500ml"),
        ("date", "2026-11-26"),
        ("conditions", "仅新客券"),
        ("currency", "USD"),
        ("priceKind", "starting_at"),
        ("amount", "299–399"),
    ] {
        let mut row = price_row("299");
        row[field] = json!(mismatch);
        let mut state = price_state();
        source(
            &mut state,
            PRICE_SOURCE,
            &json!({"offers":[row]}).to_string(),
        );
        let (reply, complete) = state.render();
        assert!(!complete, "{field}: {reply}");
        assert_eq!(state.status()["questions"][0]["state"], "unresolved");
        assert!(!reply.contains("商家列出的固定到手总价"), "{reply}");
        if field == "priceKind" {
            assert!(reply.contains("页面列出 CNY 299.00") && reply.contains("未标明固定到手总价"));
        }
    }
    let mut missing_date = price_row("299");
    missing_date.as_object_mut().unwrap().remove("date");
    let mut missing_amount = price_row("299");
    missing_amount.as_object_mut().unwrap().remove("amount");
    let mut state = price_state();
    source(
        &mut state,
        PRICE_SOURCE,
        &json!({"offers":[missing_date,missing_amount]}).to_string(),
    );
    let (reply, complete) = state.render();
    assert!(
        !complete && reply.contains("适用日期") && reply.contains("缺少有效金额"),
        "{reply}"
    );
    assert!(!reply.contains("商家列出的固定到手总价"));
}

#[test]
fn conflicting_same_scope_prices_are_exposed_and_identical_prices_do_not_vote() {
    let mut state = price_state();
    for (url, amount) in [
        (PRICE_SOURCE, "299"),
        ("https://shop.example/copy", "299.00"),
    ] {
        source(
            &mut state,
            url,
            &json!({"offers":[price_row(amount)]}).to_string(),
        );
    }
    assert!(state.render().1);
    assert_eq!(state.status()["questions"][0]["state"], "price-supported");
    source(
        &mut state,
        PRICE_UPDATE,
        &json!({"offers":[price_row("309.50")]}).to_string(),
    );
    let restored = serde_json::from_str::<State>(&serde_json::to_string(&state).unwrap())
        .unwrap()
        .restore();
    let (reply, complete) = restored.render();
    assert!(
        !complete && reply.contains("同一口径出现不同报价"),
        "{reply}"
    );
    for proof in ["CNY 299.00", "CNY 309.50", PRICE_SOURCE, PRICE_UPDATE] {
        assert!(reply.contains(proof), "{reply}");
    }
    assert_eq!(restored.status()["questions"][0]["state"], "conflicting");
    assert!(!reply.contains("/copy"));
}

#[test]
fn price_requires_an_intact_bound_feed_and_enforces_the_owners_host() {
    let text = json!({"offers":[price_row("299")]}).to_string();
    let mut state = price_state();
    source(&mut state, "https://other.example/quotes", &text);
    source(&mut state, PRICE_SOURCE, &text);
    state.observations[1].receipt = Some(WebReceipt::Page {
        requested: "https://other.example/redirect".into(),
        effective: Some(PRICE_SOURCE.into()),
        complete: true,
    });
    source(&mut state, PRICE_UPDATE, &text);
    state.observations[2].receipt = None;
    source(&mut state, "https://shop.example/bad", &text);
    state.observations[3].digest = "tampered".into();
    assert!(!state.render().1);
    assert!(!state.render().0.contains("CNY 299"));
    let exact = "299";
    let err = state
        .record(json!({
            "action":"record","question":1,"citations":[{"source":1,"quote":exact}]
        }))
        .unwrap_err();
    assert!(err.to_string().contains("host evaluates price questions"));
    let ordinary = command("/web_research\nprice trends for this SKU?", QUICK)
        .unwrap()
        .unwrap();
    assert!(ordinary.price_queries.is_empty());
    for invalid in [
        PRICE_QUERY.replace("shop.example", "shop.example/path"),
        PRICE_QUERY.replace("2026-11-27", "今天"),
        PRICE_QUERY.replace("无优惠", ""),
    ] {
        assert!(
            command(&format!("{QUICK}\n{invalid}"), QUICK)
                .unwrap()
                .is_err()
        );
    }
}

#[test]
fn both_prices_can_come_from_distinct_records_on_one_page_but_duplicate_json_fields_are_invalid() {
    let mut state = price_state();
    source(
        &mut state,
        PRICE_SOURCE,
        &json!({"offers":[price_row("299"),price_row("309")]}).to_string(),
    );
    let (reply, complete) = state.render();
    assert!(!complete && reply.contains("报价记录 1") && reply.contains("报价记录 2"));
    let mut malformed = price_state();
    let row = price_row("299").to_string().replacen(
        r#""amount":"299""#,
        r#""amount":"299","amount":"309""#,
        1,
    );
    source(
        &mut malformed,
        PRICE_SOURCE,
        &format!(r#"{{"offers":[{row}]}}"#),
    );
    assert!(!malformed.render().1);
}
