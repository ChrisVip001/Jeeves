//! Bundle tests: the claim is "plain files you can trust", so every test is about what
//! survives an edit, a crash, or a hostile slug.

use crate::bundle::{Bundle, Page};

fn temp_bundle(name: &str) -> Bundle {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path().join(name);
    let _ = dir.keep();
    let bundle = Bundle::from_root(&root);
    bundle.ensure().expect("create the store");
    bundle
}

#[test]
fn a_page_round_trips() {
    let bundle = temp_bundle("page_roundtrip");
    let mut page = Page::new(
        "alex",
        "person",
        "Alex",
        "alex",
        "likes morning meetings",
        "first line\nsecond line",
    );
    page.sources = vec!["ledger:s1/t1".to_string()];
    bundle.write(&page).expect("write the page");

    let read = bundle.read("alex").expect("read").expect("it exists");
    assert_eq!(read.kind, "person");
    assert_eq!(read.title, "Alex");
    assert_eq!(read.summary, "likes morning meetings");
    assert_eq!(read.body, "first line\nsecond line");
    assert_eq!(read.sources, vec!["ledger:s1/t1".to_string()]);
    assert_eq!(read.digest, page.digest);
}

#[test]
fn the_catalog_is_regenerated_from_the_pages() {
    let bundle = temp_bundle("page_index");
    bundle
        .write(&Page::new(
            "alex",
            "person",
            "Alex",
            "alex",
            "likes morning meetings",
            "the body",
        ))
        .expect("write");
    bundle
        .write(&Page::new(
            "jeeves",
            "project",
            "Jeeves",
            "jeeves",
            "a long-task assistant",
            "the body",
        ))
        .expect("write");

    let index = bundle.index_text().expect("the directory");
    assert!(
        index.contains("2 pages"),
        "the count has to be right: {index}"
    );
    assert!(
        index.contains("[Alex](alex.md) · person — likes morning meetings"),
        "{index}"
    );
    assert!(
        index.contains("[Jeeves](jeeves.md) · project — a long-task assistant"),
        "{index}"
    );
}

#[test]
fn the_digest_follows_a_hand_edit() {
    let bundle = temp_bundle("page_handedit");
    bundle
        .write(&Page::new(
            "alex",
            "fact",
            "Alex",
            "alex",
            "the summary",
            "the original",
        ))
        .expect("write");

    let path = bundle.root().join("alex.md");
    let edited = std::fs::read_to_string(&path)
        .expect("read")
        .replace("the original", "edited by someone");
    std::fs::write(&path, edited).expect("a hand edit");

    let read = bundle.read("alex").expect("read").expect("it exists");
    assert_eq!(read.body, "edited by someone");
    assert_ne!(
        read.digest,
        crate::bundle::digest_of("the original"),
        "the summary has to reflect the body"
    );
    assert_eq!(read.digest, crate::bundle::digest_of("edited by someone"));
}

#[test]
fn removing_a_page_leaves_a_trail() {
    let bundle = temp_bundle("page_remove");
    bundle
        .write(&Page::new("gone", "fact", "Gone", "gone", "", "the body"))
        .expect("write");
    assert!(bundle.remove("gone").expect("delete it"));
    assert!(
        !bundle.remove("gone").expect("delete it again"),
        "no longer there"
    );

    assert!(bundle.read("gone").expect("read").is_none());
    assert!(
        !bundle
            .index_text()
            .expect("the directory")
            .contains("gone.md")
    );
    let trail = std::fs::read_to_string(bundle.root().join("changelog.md")).expect("the history");
    assert!(trail.contains("write gone fact"), "{trail}");
    assert!(trail.contains("remove gone"), "{trail}");
}

#[test]
fn one_broken_page_does_not_hide_the_rest() {
    let bundle = temp_bundle("page_broken");
    bundle
        .write(&Page::new(
            "good",
            "fact",
            "Good",
            "good",
            "still there",
            "the body",
        ))
        .expect("write");
    std::fs::write(
        bundle.root().join("broken.md"),
        "a paragraph with no frontmatter\n",
    )
    .expect("write the broken file");

    let pages = bundle.pages().expect("list");
    assert_eq!(pages.len(), 1, "the bad one is skipped, the good one stays");
    assert_eq!(pages[0].slug, "good");
}

#[test]
fn slug_validation_refuses_traversal() {
    let bundle = temp_bundle("page_slug");
    for bad in [
        "../etc/passwd",
        "a/b",
        "Index",
        "",
        "index",
        "changelog",
        "with a space",
    ] {
        let error = bundle
            .page_path(bad)
            .err()
            .unwrap_or_else(|| panic!("`{bad}` has to be refused"));
        assert!(
            !error.to_string().is_empty(),
            "a refusal has to carry a reason"
        );
    }
    assert!(
        bundle.page_path("weekly-review_2").is_ok(),
        "an ordinary name has to be allowed"
    );
    assert!(
        bundle.page_path("带中文").is_ok(),
        "a CJK page name has to be allowed — a hand-written library should be readable"
    );
}

#[test]
fn a_page_file_starts_with_its_type() {
    let bundle = temp_bundle("page_shape");
    bundle
        .write(&Page::new(
            "alex",
            "person",
            "Alex",
            "alex",
            "the summary",
            "the body",
        ))
        .expect("write");
    let text = std::fs::read_to_string(bundle.root().join("alex.md")).expect("read");
    assert!(text.starts_with("---\ntype: person\n"), "{text}");
    assert!(text.contains("title: Alex\n"), "{text}");
    assert!(text.trim_end().ends_with("the body"), "{text}");
}
