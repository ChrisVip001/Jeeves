//! Frontmatter tests: the two rules that broke before — CRLF, and empty means absent.

use crate::frontmatter::{render, split};

#[test]
fn reads_a_plain_header() {
    let parsed =
        split("---\ntype: fact\ntitle: Alex\n---\n\none line of body.\n").expect("it has a header");
    assert_eq!(parsed.get("type"), Some("fact"));
    assert_eq!(parsed.get("title"), Some("Alex"));
    assert_eq!(parsed.body.trim(), "one line of body.");
}

#[test]
fn reads_a_crlf_header() {
    let parsed = split("---\r\nname: x\r\ndescription: y\r\n---\r\n\r\nbody\r\n")
        .expect("CRLF has to be accepted too");
    assert_eq!(parsed.get("name"), Some("x"));
    assert_eq!(parsed.body.trim(), "body");
}

#[test]
fn strips_quotes_from_values() {
    let parsed = split("---\ntitle: \"a quoted title\"\n---\nbody").expect("it has a header");
    assert_eq!(parsed.get("title"), Some("a quoted title"));
}

#[test]
fn an_empty_value_is_absent() {
    let parsed = split("---\nschedule:\nversion: 1.0\n---\nbody").expect("it has a header");
    assert_eq!(parsed.get("schedule"), None);
    assert_eq!(parsed.get("version"), Some("1.0"));
}

#[test]
fn flags_read_the_negative_spellings() {
    let parsed = split("---\na: false\nb: no\nc: 0\nd: true\n---\nbody").expect("it has a header");
    assert!(!parsed.flag("a", true));
    assert!(!parsed.flag("b", true));
    assert!(!parsed.flag("c", true));
    assert!(parsed.flag("d", false));
    assert!(parsed.flag("missing", true), "absent means the default");
}

#[test]
fn render_and_split_round_trip() {
    let text = render(
        &[("type", "fact".to_string()), ("title", "Alex".to_string())],
        "first line\nsecond line",
    );
    let parsed = split(&text).expect("it has a header");
    assert_eq!(parsed.get("type"), Some("fact"));
    assert_eq!(parsed.get("title"), Some("Alex"));
    assert_eq!(parsed.body.trim(), "first line\nsecond line");
}

#[test]
fn a_file_without_a_header_is_not_parsed() {
    assert!(split("just a paragraph of body\n").is_none());
    assert!(
        split("---\nkey: value\nan unclosed header").is_none(),
        "an unclosed header is broken"
    );
}
