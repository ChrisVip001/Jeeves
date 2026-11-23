use super::edit_match::{Located, Tier, fit_line_endings, lines_equivalent, locate, miss_hint};

fn one(text: &str, needle: &str) -> (String, Tier) {
    match locate(text, needle) {
        Located::One { range, tier } => (text[range].to_string(), tier),
        other => panic!("expected one match, got {other:?}"),
    }
}

#[test]
fn exact_matches_win_before_any_normalization() {
    assert_eq!(one("let a = 1;\n", "a = 1"), ("a = 1".into(), Tier::Exact));
}

#[test]
fn crlf_files_match_an_lf_old_string() {
    let text = "first\r\nsecond\r\nthird\r\n";
    let (found, tier) = one(text, "first\nsecond");
    assert_eq!(found, "first\r\nsecond");
    assert_eq!(tier, Tier::Normalized);
    assert_eq!(
        fit_line_endings(&found, "one\ntwo", tier),
        "one\r\ntwo",
        "the replacement takes the file's CRLF"
    );
}

#[test]
fn typographic_quotes_and_dashes_fold_to_ascii() {
    let text = "say \u{201C}hello\u{201D} \u{2014} it\u{2019}s fine\n";
    let (found, tier) = one(text, "\"hello\" - it's");
    assert_eq!(found, "\u{201C}hello\u{201D} \u{2014} it\u{2019}s");
    assert_eq!(tier, Tier::Normalized);
}

#[test]
fn trailing_whitespace_is_ignored_line_by_line() {
    let text = "fn main() {   \n    run();\t\n}\n";
    let (found, tier) = one(text, "fn main() {\n    run();\n");
    assert_eq!(found, "fn main() {   \n    run();\t\n");
    assert_eq!(tier, Tier::LineTrimmed);
}

#[test]
fn indentation_is_never_ignored() {
    let text = "if x:\n    run()\n";
    assert_eq!(locate(text, "if x:\nrun()"), Located::Nothing);
}

#[test]
fn ambiguity_in_a_tolerant_tier_is_still_refused() {
    let text = "a\u{2019}b\nother\na\u{2019}b\n";
    assert_eq!(
        locate(text, "a'b"),
        Located::Many {
            count: 2,
            tier: Tier::Normalized
        }
    );
}

#[test]
fn lines_equivalent_ignores_only_the_tolerated_differences() {
    assert!(lines_equivalent("x = \u{201C}a\u{201D}  ", "x = \"a\""));
    assert!(!lines_equivalent("  x", "x"));
}

#[test]
fn a_miss_shows_the_closest_numbered_lines() {
    let text = "header\nfn total(items: &[u32]) -> u32 {\n    items.iter().sum()\n}\nfooter\n";
    let old = "fn total(items:  &[u32]) -> u32 {\n  items.iter().sum()\n}";
    let hint = miss_hint(text, old, "x").expect("most lines agree on one place");
    assert!(hint.contains("lines 2-3"), "{hint}");
    assert!(hint.contains("     3\t    items.iter().sum()"), "{hint}");
    assert!(!hint.contains("header"), "{hint}");
    assert!(!hint.contains("footer"), "{hint}");
}

#[test]
fn a_miss_with_little_overlap_gives_no_location_hint() {
    let text = "alpha\nbeta\ngamma\n";
    assert_eq!(miss_hint(text, "delta\nepsilon\nalpha", "x"), None);
}

#[test]
fn a_miss_notices_an_edit_that_already_landed() {
    let text = "const LIMIT: usize = 4096; // raised\n";
    let hint = miss_hint(
        text,
        "const LIMIT: usize = 1024;",
        "const LIMIT: usize = 4096;",
    )
    .expect("new_string is already there");
    assert!(hint.contains("already be applied"), "{hint}");
}
