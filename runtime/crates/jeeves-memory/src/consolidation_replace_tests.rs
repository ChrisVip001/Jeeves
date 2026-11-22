//! Knowledge updates: a newer distilled claim retires the one it replaces, and nothing else.
use super::consolidation::consolidate_if_due;
use jeeves_provider::mock::Mock;
use jeeves_state::{Chat, Episodes, Facts};

async fn fixture() -> (tempfile::TempDir, Chat, Facts, Episodes) {
    let home = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&home.path().join("state.db"))
        .await
        .unwrap();
    (
        home,
        Chat::new(pool.clone()),
        Facts::new(pool.clone()),
        Episodes::new(pool),
    )
}

async fn distill(chat: &Chat, facts: &Facts, episodes: &Episodes, said: &str, reply: &str) -> Mock {
    chat.append_exchange(said, "Noted.", "chat", "test", None)
        .await
        .unwrap();
    let mock = Mock::new(vec![Mock::text(reply)]);
    consolidate_if_due(chat, facts, episodes, &mock, "small", 1)
        .await
        .unwrap();
    mock
}

#[tokio::test]
async fn a_shown_fact_on_the_same_subject_is_retired_and_stays_retired() {
    let (_home, chat, facts, episodes) = fixture().await;
    distill(
        &chat,
        &facts,
        &episodes,
        "I live in Beijing",
        r#"{"facts":[{"subject":"user","content":"lives in Beijing"}],"episode":"home"}"#,
    )
    .await;
    let old = facts.search("Beijing", 1).await.unwrap()[0].id;

    let mock = distill(&chat, &facts, &episodes, "I just moved to Shanghai",
        &format!(r#"{{"facts":[{{"subject":"user","content":"lives in Shanghai","replaces":[{old}]}}],"episode":"move"}}"#)).await;
    let shown = mock.received.lock().unwrap()[0].messages[0].text();
    assert!(
        shown.contains(&format!("{old}: user — lives in Beijing")),
        "{shown}"
    );

    assert!(facts.search("Beijing", 5).await.unwrap().is_empty());
    assert!(
        facts.get(old).await.unwrap().is_some(),
        "the old claim stays as history"
    );
    distill(
        &chat,
        &facts,
        &episodes,
        "anything else",
        r#"{"facts":[{"subject":"user","content":"lives in Beijing"}],"episode":"again"}"#,
    )
    .await;
    assert!(facts.search("Beijing", 5).await.unwrap().is_empty());
}

#[tokio::test]
async fn ids_not_shown_other_subjects_and_human_facts_are_left_alone() {
    let (_home, chat, facts, episodes) = fixture().await;
    let (human, _) = facts
        .add("user", "lives in Beijing", "user", "user")
        .await
        .unwrap();
    distill(
        &chat,
        &facts,
        &episodes,
        "My project is Atlas",
        r#"{"facts":[{"subject":"project","content":"is called Atlas"}],"episode":"p"}"#,
    )
    .await;
    let other = facts.search("Atlas", 1).await.unwrap()[0].id;

    distill(&chat, &facts, &episodes, "I just moved to Shanghai",
        &format!(r#"{{"facts":[{{"subject":"user","content":"lives in Shanghai","replaces":[{},{other},9999]}}],"episode":"move"}}"#, human.id)).await;

    assert_eq!(
        facts.search("Beijing", 5).await.unwrap().len(),
        1,
        "a human's fact is theirs to change"
    );
    assert_eq!(
        facts.search("Atlas", 5).await.unwrap().len(),
        1,
        "another subject is not replaced"
    );
    assert_eq!(facts.search("Shanghai", 5).await.unwrap().len(), 1);
}
