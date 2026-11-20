//! Mirror tests: the two promises — one page per subject, and manual edits survive.

use jeeves_state::Facts;

use crate::bundle::Bundle;
use crate::mirror::mirror_facts;

async fn fixtures(name: &str) -> (Facts, Bundle) {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join(name);
    std::fs::create_dir_all(&home).expect("create the directory");
    let _ = dir.keep();
    let pool = jeeves_state::open(&home.join("state.db"))
        .await
        .expect("open the store");
    (Facts::new(pool), Bundle::new(&home))
}

#[tokio::test]
async fn a_subject_becomes_one_page() {
    let (facts, bundle) = fixtures("mirror_subject").await;
    let (first, _) = facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .expect("write");
    let _ = facts
        .add("alex", "lives in Hangzhou", "user", "user")
        .await
        .expect("write");

    let report = mirror_facts(&facts, &bundle).await.expect("the mirror");
    assert_eq!(report.written, 1);
    assert_eq!(report.skipped, 0);

    let page = bundle.read("alex").expect("read").expect("there is a page");
    assert_eq!(page.title, "alex");
    assert!(
        page.body.contains("likes morning meetings"),
        "{}",
        page.body
    );
    assert!(page.body.contains("lives in Hangzhou"), "{}", page.body);
    assert!(
        page.sources
            .iter()
            .any(|source| source == &format!("fact:{first}", first = first.id)),
        "the provenance has to point back at the fact row"
    );
    assert!(bundle
        .index_text()
        .expect("the directory")
        .contains("alex.md"));

    let meta = facts.meta(first.id).await.expect("query").expect("present");
    assert_eq!(meta.page_path.as_deref(), Some("memory/alex.md"));
    assert_eq!(meta.digest.as_deref(), Some(page.digest.as_str()));
}

#[tokio::test]
async fn a_hand_edited_page_is_not_overwritten() {
    let (facts, bundle) = fixtures("mirror_handedit").await;
    facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .expect("write");
    mirror_facts(&facts, &bundle)
        .await
        .expect("the first mirror");

    let path = bundle.root().join("alex.md");
    let edited = std::fs::read_to_string(&path)
        .expect("read")
        .replace("likes morning meetings", "changed to after dinner");
    std::fs::write(&path, &edited).expect("a hand edit");

    facts
        .add("alex", "keeps a cat", "user", "user")
        .await
        .expect("write");
    let report = mirror_facts(&facts, &bundle)
        .await
        .expect("the second mirror");
    assert_eq!(report.written, 0, "it must not overwrite");
    assert_eq!(report.skipped, 1, "the skip should be recorded");

    let page = bundle.read("alex").expect("read").expect("there is a page");
    assert!(
        page.body.contains("changed to after dinner"),
        "the human's edit has to survive"
    );
    assert!(
        !page.body.contains("keeps a cat"),
        "the new fact did not reach the file this time"
    );
}

#[tokio::test]
async fn an_empty_store_writes_nothing() {
    let (facts, bundle) = fixtures("mirror_empty").await;
    let report = mirror_facts(&facts, &bundle).await.expect("the mirror");
    assert_eq!(report.written, 0);
    assert!(bundle.pages().expect("list").is_empty());
}

#[tokio::test]
async fn a_chinese_subject_gets_a_stable_slug() {
    let (facts, bundle) = fixtures("mirror_cjk").await;
    facts
        .add("张伟", "works in Shanghai", "user", "user")
        .await
        .expect("write");
    mirror_facts(&facts, &bundle).await.expect("the mirror");

    let pages = bundle.pages().expect("list");
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].slug, "张伟", "a CJK subject keeps a CJK page name");

    let again = mirror_facts(&facts, &bundle).await.expect("mirror again");
    assert_eq!(again.written, 1);
    assert_eq!(
        bundle.pages().expect("list").len(),
        1,
        "a second page must not appear"
    );
}

#[tokio::test]
async fn an_existing_untracked_page_is_never_claimed_as_generated_content() {
    let (facts, bundle) = fixtures("mirror_untracked").await;
    bundle.ensure().unwrap();
    let page = crate::bundle::Page::new(
        "alex",
        "fact",
        "Alex",
        "Alex",
        "Human note",
        "Keep this human-authored context.",
    );
    bundle.write(&page).unwrap();
    facts
        .add("alex", "likes morning meetings", "user", "user")
        .await
        .unwrap();
    let report = mirror_facts(&facts, &bundle).await.unwrap();
    assert_eq!(report.written, 0);
    assert_eq!(report.skipped, 1);
    assert_eq!(bundle.read("alex").unwrap().unwrap().body, page.body);
}

#[tokio::test]
async fn stale_pages_are_excluded_immediately_without_waiting_for_the_worker() {
    let (facts, bundle) = fixtures("stale_recall").await;
    let (first, _) = facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    facts
        .add("alex", "likes walks", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(super::page_is_current(&facts, &page).await.unwrap());
    facts
        .set_validity(first.id, None, Some("2000-01-01T00:00:00Z"))
        .await
        .unwrap();
    assert!(!super::page_is_current(&facts, &page).await.unwrap());
    assert!(!super::recall_catalog(&facts, &bundle)
        .await
        .unwrap()
        .contains("alex"));
    facts.set_validity(first.id, None, None).await.unwrap();
    assert!(super::page_is_current(&facts, &page).await.unwrap());
    facts.update(first.id, "likes coffee").await.unwrap();
    assert!(!super::page_is_current(&facts, &page).await.unwrap());
    facts.delete(first.id).await.unwrap();
    assert!(!super::page_is_current(&facts, &page).await.unwrap());
    assert!(bundle.read("alex").unwrap().is_some());
}

use crate::mirror::sync_edited_pages;

fn edit_page(bundle: &Bundle, slug: &str, from: &str, to: &str) {
    let path = bundle.root().join(format!("{slug}.md"));
    let text = std::fs::read_to_string(&path).expect("read the page");
    assert!(
        text.contains(from),
        "the page should contain {from:?}:\n{text}"
    );
    std::fs::write(&path, text.replacen(from, to, 1)).expect("hand edit");
}

#[tokio::test]
async fn every_bullet_carries_an_invisible_trailer() {
    let (facts, bundle) = fixtures("sync_trailer").await;
    let (row, _) = facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(
        page.body.contains(&format!(
            "- likes tea <!-- fact:{} kind=user source=user at=",
            row.id
        )),
        "{}",
        page.body
    );
    assert!(super::page_is_current(&facts, &page).await.unwrap());
}

#[tokio::test]
async fn rewriting_a_line_updates_that_fact_and_the_old_wording_stays_gone() {
    let (facts, bundle) = fixtures("sync_update").await;
    let (row, _) = facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    facts
        .add("alex", "keeps a cat", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();

    edit_page(&bundle, "alex", "- likes tea", "- likes coffee");
    let report = sync_edited_pages(&facts, &bundle).await.unwrap();
    assert_eq!((report.updated, report.deleted, report.added), (1, 0, 0));

    let rows = facts.for_subject("alex").await.unwrap();
    assert!(rows
        .iter()
        .any(|r| r.id == row.id && r.content == "likes coffee"));
    assert!(facts
        .add_consolidated("alex", "likes tea", "fact")
        .await
        .unwrap()
        .is_none());
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(page.body.contains("likes coffee <!--"), "{}", page.body);
    assert!(super::page_is_current(&facts, &page).await.unwrap());
    assert!(!sync_edited_pages(&facts, &bundle).await.unwrap().changed());
}

#[tokio::test]
async fn deleting_a_line_deletes_the_fact_and_the_mirror_does_not_write_it_back() {
    let (facts, bundle) = fixtures("sync_delete").await;
    facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    facts
        .add("alex", "keeps a cat", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();

    let path = bundle.root().join("alex.md");
    let text = std::fs::read_to_string(&path).unwrap();
    let kept: Vec<_> = text
        .lines()
        .filter(|line| !line.contains("keeps a cat"))
        .collect();
    std::fs::write(&path, kept.join("\n")).unwrap();

    let report = sync_edited_pages(&facts, &bundle).await.unwrap();
    assert_eq!(report.deleted, 1);
    mirror_facts(&facts, &bundle).await.unwrap();
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(!page.body.contains("keeps a cat"), "{}", page.body);
    assert!(facts
        .add_consolidated("alex", "keeps a cat", "fact")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn a_new_line_becomes_a_fact_said_by_the_user() {
    let (facts, bundle) = fixtures("sync_add").await;
    facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    edit_page(
        &bundle,
        "alex",
        "\n\n## Sources",
        "\n- moved to Berlin\n\n## Sources",
    );

    let report = sync_edited_pages(&facts, &bundle).await.unwrap();
    assert_eq!(report.added, 1);
    let rows = facts.for_subject("alex").await.unwrap();
    let added = rows
        .iter()
        .find(|r| r.content == "moved to Berlin")
        .expect("added");
    assert_eq!(added.source, "user");
}

#[tokio::test]
async fn deleting_every_line_removes_the_page() {
    let (facts, bundle) = fixtures("sync_empty").await;
    facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    let path = bundle.root().join("alex.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.lines()
            .filter(|l| !l.contains("likes tea"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();

    sync_edited_pages(&facts, &bundle).await.unwrap();
    assert!(facts.for_subject("alex").await.unwrap().is_empty());
    assert!(
        bundle.read("alex").unwrap().is_none(),
        "an empty page is removed, not left blank"
    );
}

#[tokio::test]
async fn a_page_with_prose_keeps_its_text_while_its_facts_sync() {
    let (facts, bundle) = fixtures("sync_prose").await;
    facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    edit_page(&bundle, "alex", "- likes tea", "- likes green tea");
    edit_page(
        &bundle,
        "alex",
        "# alex\n",
        "# alex\n\nMet at the 2024 offsite; prefers async.\n",
    );

    let report = sync_edited_pages(&facts, &bundle).await.unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(
        report.rewritten, 0,
        "a page with a human's own prose is not regenerated"
    );
    let page = bundle.read("alex").unwrap().unwrap();
    assert!(page.body.contains("Met at the 2024 offsite"));
    assert!(facts
        .for_subject("alex")
        .await
        .unwrap()
        .iter()
        .any(|r| r.content == "likes green tea"));
}

#[tokio::test]
async fn a_page_without_trailers_is_matched_by_text() {
    let (facts, bundle) = fixtures("sync_legacy").await;
    let (row, _) = facts
        .add("alex", "likes tea", "user", "user")
        .await
        .unwrap();
    mirror_facts(&facts, &bundle).await.unwrap();
    let path = bundle.root().join("alex.md");
    let text = std::fs::read_to_string(&path).unwrap();
    let stripped: Vec<String> = text
        .lines()
        .map(|line| match line.find(" <!-- fact:") {
            Some(at) => line[..at].to_string(),
            None => line.to_string(),
        })
        .collect();
    assert!(text.contains(&format!("<!-- fact:{} ", row.id)));
    std::fs::write(&path, stripped.join("\n")).unwrap();
    let legacy = bundle.read("alex").unwrap().unwrap();
    facts
        .set_page(row.id, "memory/alex.md", &legacy.digest)
        .await
        .unwrap();
    assert!(
        super::page_is_current(&facts, &legacy).await.unwrap(),
        "legacy pages stay current"
    );

    edit_page(&bundle, "alex", "- likes tea", "- likes oolong");
    let report = sync_edited_pages(&facts, &bundle).await.unwrap();
    assert_eq!((report.deleted, report.added), (1, 1));
    assert!(facts
        .for_subject("alex")
        .await
        .unwrap()
        .iter()
        .any(|r| r.content == "likes oolong"));
}

#[tokio::test]
async fn untracked_human_pages_stay_out_of_the_facts() {
    let (facts, bundle) = fixtures("sync_untracked").await;
    bundle.ensure().unwrap();
    let page = crate::bundle::Page::new(
        "notes",
        "concept",
        "Notes",
        "notes",
        "mine",
        "- a private thought",
    );
    bundle.write(&page).unwrap();
    assert!(!sync_edited_pages(&facts, &bundle).await.unwrap().changed());
    assert!(facts.for_subject("notes").await.unwrap().is_empty());
}
