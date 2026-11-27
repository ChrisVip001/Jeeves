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
        command("/web_research\n A?\n B?", QUICK).unwrap().unwrap(),
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
