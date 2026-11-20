//! Turn human input into a query FTS5 can safely digest.

/// Users type anything: quotes, asterisks, colons, half a bracket. Splicing it straight into
/// `MATCH` throws SQLite syntax errors — and the retrieval gate is **fail-open**, one error
/// degrades into "search every turn". So the input is split into words, each quoted, joined
/// with OR: always-legal grammar; any word matching suffices ("recall something related").
///
/// `None` means no usable words (all punctuation / empty); the caller should skip search.
///
/// The tokenizer is trigram (see migrations/0002): indexing by 3-char slices, CJK works
/// naturally. Words shorter than 3 chars yield no trigrams — [`like_pattern`] covers those.
pub fn to_match_expr(input: &str) -> Option<String> {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();

    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" OR "))
    }
}

/// The fallback when FTS misses: an ordinary LIKE substring scan.
///
/// Covers two cases: query terms shorter than 3 chars (two-char CJK words are common; no
/// trigrams), and trigram's inability on very short English words. A personal memory store
/// is a few thousand rows; one full LIKE scan is microseconds — no external tokenizer needed.
///
/// The wildcards `%` `_` are stripped, or a user could turn LIKE into a full-table scan
/// (only slow, but no reason to leave that door open).
pub fn like_pattern(input: &str) -> Option<String> {
    let needle: String = input.chars().filter(|c| !matches!(c, '%' | '_')).collect();
    let needle = needle.trim();
    if !needle.chars().any(char::is_alphanumeric) {
        None
    } else {
        Some(format!("%{needle}%"))
    }
}

/// Han, kana and Hangul: scripts written without spaces between words.
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{30FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}')
}

/// Most terms a CJK query is cut into; a pasted paragraph must not build a huge `WHERE`.
const MAX_TERMS: usize = 24;

/// Search terms for a query written in a spaceless script; `None` when it has no CJK at all.
///
/// A Chinese question arrives as one long "word" (`阿明能吃花生酱吗`): searched whole it matches
/// nothing, and trigrams of it rarely line up with a stored sentence. Overlapping two-character
/// pieces do (`花生` is in both), the same unit Chinese words mostly have. Non-CJK words in the
/// query are kept whole. Callers count how many distinct terms each row contains ([`hits`]).
pub fn cjk_terms(input: &str) -> Option<Vec<String>> {
    if !input.chars().any(is_cjk) {
        return None;
    }
    let mut terms: Vec<String> = Vec::new();
    for word in input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        let mut chars = word.chars();
        let first = chars.next().expect("nonempty word");
        if !word.chars().any(is_cjk) || chars.clone().next().is_none() {
            let term = word.to_lowercase();
            if !terms.contains(&term) {
                terms.push(term);
            }
        } else {
            let mut previous = first;
            for next in chars {
                let pair = String::from_iter([previous, next]);
                if !terms.contains(&pair) {
                    terms.push(pair);
                    if terms.len() == MAX_TERMS {
                        return Some(terms);
                    }
                }
                previous = next;
            }
        }
        if terms.len() == MAX_TERMS {
            break;
        }
    }
    Some(terms)
}

/// `(a LIKE ? OR b LIKE ?) OR …` for every term, to bind with [`like_binds`].
pub fn like_any(columns: &[&str], terms: usize) -> String {
    let one = columns
        .iter()
        .map(|column| format!("{column} LIKE ?"))
        .collect::<Vec<_>>()
        .join(" OR ");
    vec![format!("({one})"); terms].join(" OR ")
}

/// The patterns for [`like_any`], in bind order.
pub fn like_binds(columns: usize, terms: &[String]) -> Vec<String> {
    terms
        .iter()
        .flat_map(|term| std::iter::repeat_n(format!("%{term}%"), columns))
        .collect()
}

/// How many distinct terms `text` contains — the rank for a CJK query.
pub fn hits(text: &str, terms: &[String]) -> usize {
    let text = text.to_lowercase();
    terms
        .iter()
        .filter(|term| text.contains(term.as_str()))
        .count()
}

#[cfg(test)]
mod fts_tests {
    use super::{like_pattern, to_match_expr};

    #[test]
    fn quotes_each_token() {
        assert_eq!(
            to_match_expr("alex morning meetings").as_deref(),
            Some("\"alex\" OR \"morning\" OR \"meetings\"")
        );
    }

    #[test]
    fn strips_punctuation_that_would_break_the_grammar() {
        for bad in [
            "what's on?",
            "a:b",
            "\"quoted\"",
            "star*",
            "(unclosed",
            "--",
            "a/b",
        ] {
            if let Some(expr) = to_match_expr(bad) {
                for token in expr.split(" OR ") {
                    let inside = token.trim_matches('"');
                    for dangerous in ['(', ')', ':', '*', '"', '/', '-'] {
                        assert!(
                            !inside.contains(dangerous),
                            "{token} in the {expr} generated from {bad:?} still contains {dangerous}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn splits_on_inner_punctuation_too() {
        assert_eq!(to_match_expr("a:b").as_deref(), Some("\"a\" OR \"b\""));
    }

    #[test]
    fn a_chinese_question_becomes_overlapping_pairs() {
        assert_eq!(
            super::cjk_terms("阿明 吃 alex?").unwrap(),
            vec!["阿明", "吃", "alex"]
        );
        assert_eq!(super::cjk_terms("花生酱").unwrap(), vec!["花生", "生酱"]);
        assert_eq!(super::cjk_terms("morning meetings"), None);
        assert_eq!(
            super::hits("对花生过敏", &["花生".into(), "生酱".into()]),
            1
        );
    }

    #[test]
    fn cjk_queries_keep_the_first_distinct_pairs_within_the_sql_limit() {
        let query = "天地天地玄黃宇宙洪荒日月盈昃辰宿列張寒來暑往秋收冬藏閏餘成歲律呂調陽";
        let terms = super::cjk_terms(query).unwrap();
        assert_eq!(terms.len(), super::MAX_TERMS);
        assert_eq!(&terms[..4], ["天地", "地天", "地玄", "玄黃"]);
        assert!(!terms.contains(&"調陽".to_string()));
    }

    #[test]
    fn punctuation_only_is_none() {
        assert_eq!(to_match_expr("???  ...  "), None);
        assert_eq!(to_match_expr(""), None);
        assert_eq!(like_pattern("???"), None);
    }

    #[test]
    fn like_pattern_keeps_chinese_and_drops_wildcards() {
        assert_eq!(like_pattern("早上").as_deref(), Some("%早上%"));
        assert_eq!(like_pattern("a%b").as_deref(), Some("%ab%"));
    }
}
