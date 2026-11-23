//! Parity: the new write entry and the old "write the table, mirror later" path must leave the
//! same pages behind. Plus the things only `apply` does (forget removes the page).

use jeeves_state::Facts;

use crate::bundle::Bundle;
use crate::mirror::{mirror_facts, reindex_from_pages};
use crate::store::{Change, Outcome, apply};

async fn home(name: &str) -> (Facts, Bundle) {
    let dir = tempfile::tempdir().expect("temp dir").keep().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let pool = jeeves_state::open(&dir.join("state.db")).await.unwrap();
    (Facts::new(pool), Bundle::new(&dir))
}

fn pages(bundle: &Bundle) -> Vec<(String, String)> {
    bundle
        .pages()
        .unwrap()
        .into_iter()
        .map(|page| {
            let body = page
                .body
                .lines()
                .map(|line| match line.find(" at=") {
                    Some(at) => format!("{}{}", &line[..at], " at=_ -->"),
                    None => line.split(" · 20").next().unwrap_or(line).to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            (page.slug, body)
        })
        .collect()
}

#[tokio::test]
async fn apply_and_the_old_path_leave_the_same_pages() {
    let (new_facts, new_bundle) = home("parity_new").await;
    let (old_facts, old_bundle) = home("parity_old").await;

    let Outcome::Remembered { row: tea, .. } = apply(
        &new_facts,
        &new_bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "user",
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    apply(
        &new_facts,
        &new_bundle,
        Change::Remember {
            subject: "alex",
            content: "keeps a cat",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    apply(
        &new_facts,
        &new_bundle,
        Change::Remember {
            subject: "acme",
            content: "ships in May",
            source: "user",
            kind: "project",
        },
    )
    .await
    .unwrap();
    apply(
        &new_facts,
        &new_bundle,
        Change::Update {
            id: tea.id,
            content: "likes coffee",
        },
    )
    .await
    .unwrap();

    let (old_tea, _) = old_facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    old_facts
        .add("alex", "keeps a cat", "user", "fact")
        .await
        .unwrap();
    old_facts
        .add("acme", "ships in May", "user", "project")
        .await
        .unwrap();
    old_facts.update(old_tea.id, "likes coffee").await.unwrap();
    mirror_facts(&old_facts, &old_bundle).await.unwrap();

    assert_eq!(pages(&new_bundle), pages(&old_bundle));
}

#[tokio::test]
async fn a_write_reaches_the_page_at_once() {
    let (facts, bundle) = home("apply_now").await;
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    let page = bundle
        .read("alex")
        .unwrap()
        .expect("written without waiting for the worker");
    assert!(page.body.contains("likes tea"));
    assert!(crate::mirror::page_is_current(&facts, &page).await.unwrap());
}

#[tokio::test]
async fn forgetting_a_subject_removes_its_page() {
    let (facts, bundle) = home("apply_forget").await;
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    let out = apply(&facts, &bundle, Change::ForgetSubject { subject: "alex" })
        .await
        .unwrap();
    assert_eq!(out, Outcome::Forgot(1));
    assert!(
        bundle.read("alex").unwrap().is_none(),
        "a forgotten subject must not linger on disk"
    );
}

#[tokio::test]
async fn a_pending_hand_edit_is_taken_before_the_write() {
    let (facts, bundle) = home("apply_race").await;
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    let path = bundle.root().join("alex.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replacen("- likes tea", "- likes green tea", 1)).unwrap();

    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "keeps a cat",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    let body = bundle.read("alex").unwrap().unwrap().body;
    assert!(
        body.contains("likes green tea"),
        "the human's edit survives the write:\n{body}"
    );
    assert!(body.contains("keeps a cat"));
}

#[tokio::test]
async fn a_page_with_prose_is_never_overwritten_by_a_write() {
    let (facts, bundle) = home("apply_prose").await;
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    let path = bundle.root().join("alex.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replacen("# alex\n", "# alex\n\nMy own notes.\n", 1),
    )
    .unwrap();

    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "keeps a cat",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    assert!(
        bundle
            .read("alex")
            .unwrap()
            .unwrap()
            .body
            .contains("My own notes.")
    );
    assert!(
        facts
            .for_subject("alex")
            .await
            .unwrap()
            .iter()
            .any(|r| r.content == "keeps a cat")
    );
}

#[tokio::test]
async fn the_index_rebuilds_from_pages_under_the_same_ids() {
    let dir = tempfile::tempdir().unwrap();
    let pool = jeeves_state::open(&dir.path().join("state.db"))
        .await
        .unwrap();
    let (facts, bundle) = (Facts::new(pool.clone()), Bundle::new(dir.path()));
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "user",
        },
    )
    .await
    .unwrap();
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "keeps a cat",
            source: "consolidation",
            kind: "fact",
        },
    )
    .await
    .unwrap();
    apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "acme",
            content: "ships in May",
            source: "user",
            kind: "project",
        },
    )
    .await
    .unwrap();
    let before = facts.all_by_subject(100).await.unwrap();

    drop(facts);
    pool.close().await;
    let facts = Facts::new(
        jeeves_state::open(&dir.path().join("restored.db"))
            .await
            .unwrap(),
    );
    assert_eq!(reindex_from_pages(&facts, &bundle).await.unwrap(), 3);

    let after = facts.all_by_subject(100).await.unwrap();
    let shape = |rows: &[jeeves_state::FactRow]| {
        let mut rows: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.id,
                    r.subject.clone(),
                    r.content.clone(),
                    r.source.clone(),
                    r.kind.clone(),
                    r.created_at.clone(),
                )
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(shape(&before), shape(&after), "row for row, ids included");
    for page in bundle.pages().unwrap() {
        assert!(crate::mirror::page_is_current(&facts, &page).await.unwrap());
    }
    assert_eq!(
        reindex_from_pages(&facts, &bundle).await.unwrap(),
        0,
        "idempotent"
    );
}

#[tokio::test]
async fn a_deleted_fact_is_not_restored_from_a_page_that_still_lists_it() {
    let (facts, bundle) = home("reindex_suppressed").await;
    let Outcome::Remembered { row, .. } = apply(
        &facts,
        &bundle,
        Change::Remember {
            subject: "alex",
            content: "likes tea",
            source: "user",
            kind: "fact",
        },
    )
    .await
    .unwrap() else {
        panic!()
    };
    facts
        .add("alex", "keeps a cat", "user", "fact")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    facts.delete(row.id).await.unwrap();
    assert_eq!(reindex_from_pages(&facts, &bundle).await.unwrap(), 0);
    assert!(facts.get(row.id).await.unwrap().is_none());
}
