use super::*;

fn source(state: &mut State, url: &str, text: &str) {
    state.observations.push(Observation {
        url: url.into(),
        text: text.into(),
        digest: jeeves_state::ledger::digest_of(text.as_bytes()),
        fetched_at: "2026-11-26T10:00:00Z".into(),
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
