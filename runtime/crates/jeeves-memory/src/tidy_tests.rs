//! Tidying removes only consolidation's redundant lines, and changes nothing when unsure.
use super::after_consolidation;
use crate::bundle::Bundle;
use crate::mirror::mirror_facts;
use jeeves_provider::mock::Mock;
use jeeves_state::Facts;

async fn page() -> (tempfile::TempDir, Facts, Bundle, Vec<i64>) {
    let home = tempfile::tempdir().unwrap();
    let facts = Facts::new(
        jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap(),
    );
    let mut ids = Vec::new();
    for content in ["likes tea", "prefers tea", "works at Acme", "has a dog"] {
        let (row, _) = facts
            .add_consolidated_from("alex", content, "fact", &[])
            .await
            .unwrap()
            .unwrap();
        ids.push(row.id);
    }
    let (human, _) = facts
        .add("alex", "drinks tea daily", "user", "fact")
        .await
        .unwrap();
    ids.push(human.id);
    let bundle = Bundle::new(home.path());
    mirror_facts(&facts, &bundle).await.unwrap();
    (home, facts, bundle, ids)
}

fn contents(rows: &[jeeves_state::FactRow]) -> Vec<&str> {
    rows.iter().map(|row| row.content.as_str()).collect()
}

#[tokio::test]
async fn a_redundant_line_goes_from_the_facts_and_the_page() {
    let (_home, facts, bundle, ids) = page().await;
    let mock = Mock::new(vec![Mock::text(&format!(
        r#"{{"drop":[{{"id":{},"keptBy":{}}}]}}"#,
        ids[1], ids[0]
    ))]);
    assert_eq!(
        after_consolidation(&facts, &bundle, &mock, "small", 1)
            .await
            .unwrap(),
        1
    );
    let shown = mock.received.lock().unwrap()[0].messages[0].text();
    assert!(
        shown.contains(&format!("{}: drinks tea daily (kept)", ids[4])),
        "{shown}"
    );

    let rows = facts.for_subject("alex").await.unwrap();
    assert!(!contents(&rows).contains(&"prefers tea"));
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(!page.body.contains("prefers tea"), "{}", page.body);
    assert!(
        facts
            .add_consolidated_from("alex", "prefers tea", "fact", &[])
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn human_lines_unknown_ids_and_chains_are_refused() {
    let (_home, facts, bundle, ids) = page().await;
    let reply = format!(
        r#"{{"drop":[{{"id":{h},"keptBy":{a}}},{{"id":9999,"keptBy":{a}}},{{"id":{a},"keptBy":{a}}},{{"id":{b},"keptBy":{a}}},{{"id":{a},"keptBy":{c}}}]}}"#,
        h = ids[4],
        a = ids[0],
        b = ids[1],
        c = ids[2]
    );
    let mock = Mock::new(vec![Mock::text(&reply)]);
    assert_eq!(
        after_consolidation(&facts, &bundle, &mock, "small", 1)
            .await
            .unwrap(),
        1
    );
    let rows = facts.for_subject("alex").await.unwrap();
    assert_eq!(
        contents(&rows),
        [
            "likes tea",
            "works at Acme",
            "has a dog",
            "drinks tea daily"
        ]
    );
}

#[tokio::test]
async fn an_unusable_answer_or_a_hand_edited_page_changes_nothing() {
    let (_home, facts, bundle, _) = page().await;
    let mock = Mock::new(vec![Mock::text("I would rather not.")]);
    assert_eq!(
        after_consolidation(&facts, &bundle, &mock, "small", 1)
            .await
            .unwrap(),
        0
    );
    assert_eq!(facts.for_subject("alex").await.unwrap().len(), 5);

    let path = bundle.page_path("alex").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("# alex\n", "# alex\n\nMy own note.\n")).unwrap();
    let mock = Mock::new(vec![]);
    assert_eq!(
        after_consolidation(&facts, &bundle, &mock, "small", 1)
            .await
            .unwrap(),
        0
    );
    assert!(
        mock.received.lock().unwrap().is_empty(),
        "a hand-edited page is not even asked about"
    );
}
