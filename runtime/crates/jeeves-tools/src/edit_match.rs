//! Locating `edit`'s `old_string` in a file, and explaining a miss.
//!
//! Models copy text out of `read_file` and lose details on the way: CRLF becomes LF, a curly
//! quote becomes a straight one, trailing spaces vanish. An exact-only match turns each of those
//! into a failed call and another round trip. Matching runs in three tiers and stops at the
//! first tier that finds anything:
//!
//! 1. **Exact** bytes.
//! 2. **Normalized**: CRLF folded to LF, typographic quotes, dashes and special spaces folded to
//!    their ASCII forms. Each fold is one character for one character (or drops the `\r` of a
//!    CRLF), so a match maps back to an exact byte range of the original.
//! 3. **Whole lines, trailing whitespace ignored**, on top of the normalization.
//!
//! Leading whitespace is never ignored: indentation is meaning in Python and YAML. Several
//! matches in the first tier that finds any is still a refusal, exactly like the exact tier.
//! A tolerant match is reported to the model, never applied silently.

use std::collections::HashMap;
use std::ops::Range;

/// Lines shown in a miss hint, and the characters kept of each.
const HINT_LINES: usize = 12;
const HINT_LINE_CHARS: usize = 200;

/// A `new_string` this long found verbatim in the file means the edit probably landed already.
const ALREADY_APPLIED_MIN_CHARS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Exact,
    Normalized,
    LineTrimmed,
}

impl Tier {
    /// The sentence the tool result carries when this tier did the matching.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Tier::Exact => None,
            Tier::Normalized => Some(
                "old_string matched after normalizing line endings, typographic quotes, dashes and special spaces",
            ),
            Tier::LineTrimmed => Some(
                "old_string matched whole lines after normalizing line endings, typographic quotes, dashes, special spaces and trailing whitespace",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    One { range: Range<usize>, tier: Tier },
    Many { count: usize, tier: Tier },
    Nothing,
}

/// Find `needle` in `text`, tier by tier. Ranges are byte ranges of `text`.
pub fn locate(text: &str, needle: &str) -> Located {
    let exact: Vec<Range<usize>> = text
        .match_indices(needle)
        .map(|(at, found)| at..at + found.len())
        .collect();
    if let Some(found) = settle(exact, Tier::Exact) {
        return found;
    }
    let view = View::new(text);
    let wanted = View::new(needle).text;
    if !wanted.is_empty() {
        let normalized: Vec<Range<usize>> = view
            .text
            .match_indices(&wanted)
            .map(|(at, found)| view.original(at..at + found.len()))
            .collect();
        if let Some(found) = settle(normalized, Tier::Normalized) {
            return found;
        }
    }
    settle(line_matches(text, needle), Tier::LineTrimmed).unwrap_or(Located::Nothing)
}

fn settle(hits: Vec<Range<usize>>, tier: Tier) -> Option<Located> {
    match hits.len() {
        0 => None,
        1 => hits
            .into_iter()
            .next()
            .map(|range| Located::One { range, tier }),
        count => Some(Located::Many { count, tier }),
    }
}

/// The replacement as it should be written over `region`. A tolerant match into a CRLF region
/// with an LF-only replacement takes the region's CRLF, so one edit never mixes line endings.
pub fn fit_line_endings(region: &str, replacement: &str, tier: Tier) -> String {
    if tier != Tier::Exact && region.contains("\r\n") && !replacement.contains('\r') {
        replacement.replace('\n', "\r\n")
    } else {
        replacement.to_string()
    }
}

/// Fold one character to its plain form. Always one character for one character.
fn fold(c: char) -> char {
    match c {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        _ => c,
    }
}

/// Normalized text plus, for every normalized byte, the original byte it came from.
struct View {
    text: String,
    origin: Vec<usize>,
}

impl View {
    fn new(source: &str) -> Self {
        let mut text = String::with_capacity(source.len());
        let mut origin = Vec::with_capacity(source.len() + 1);
        let mut chars = source.char_indices().peekable();
        while let Some((at, c)) = chars.next() {
            if c == '\r' && matches!(chars.peek(), Some((_, '\n'))) {
                chars.next();
                text.push('\n');
                origin.push(at);
                continue;
            }
            let folded = fold(c);
            text.push(folded);
            origin.extend(std::iter::repeat_n(at, folded.len_utf8()));
        }
        origin.push(source.len());
        Self { text, origin }
    }

    fn original(&self, range: Range<usize>) -> Range<usize> {
        self.origin[range.start]..self.origin[range.end]
    }
}

/// The comparison key of one line for the line tier.
fn loose_line(line: &str) -> String {
    let line = line.trim_suffix("\r");
    line.chars()
        .map(fold)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// Whether two lines are equal for the tolerant tiers. `apply_patch` shares this rule.
pub fn lines_equivalent(actual: &str, expected: &str) -> bool {
    actual == expected || loose_line(actual) == loose_line(expected)
}

/// `(start, content end, next line start)` for each line of `text`.
fn line_spans(text: &str) -> Vec<(usize, usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        let next = start + line.len();
        let content = line.trim_suffix("\n");
        let content = content.trim_suffix("\r");
        spans.push((start, start + content.len(), next));
        start = next;
    }
    spans
}

fn line_matches(text: &str, needle: &str) -> Vec<Range<usize>> {
    let takes_newline = needle.ends_with('\n');
    let body = needle.trim_suffix("\n");
    let body = body.trim_suffix("\r");
    let wanted: Vec<String> = body.split('\n').map(loose_line).collect();
    if wanted.iter().all(String::is_empty) {
        return Vec::new();
    }
    let spans = line_spans(text);
    let have: Vec<String> = spans
        .iter()
        .map(|(start, end, _)| loose_line(&text[*start..*end]))
        .collect();
    let mut hits = Vec::new();
    let mut at = 0;
    while at + wanted.len() <= spans.len() {
        if have[at..at + wanted.len()] == wanted[..] {
            let last = spans[at + wanted.len() - 1];
            let end = if takes_newline { last.2 } else { last.1 };
            hits.push(spans[at].0..end);
            at += wanted.len();
        } else {
            at += 1;
        }
    }
    hits
}

/// What to tell the model when nothing matched: the edit may already be in place, or the closest
/// stretch of the file, numbered like `read_file`. Only lines that matched are shown, so a long
/// `old_string` cannot pull more of the file into the reply than it overlaps.
pub fn miss_hint(text: &str, old_string: &str, new_string: &str) -> Option<String> {
    if new_string.trim().chars().count() >= ALREADY_APPLIED_MIN_CHARS && text.contains(new_string) {
        return Some(
            "new_string already appears in the file, so this edit may already be applied — read_file to check before retrying.".to_string(),
        );
    }
    let collapse = |line: &str| line.split_whitespace().collect::<Vec<_>>().join(" ");
    let file_lines: Vec<&str> = text.lines().collect();
    let mut by_content: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, line) in file_lines.iter().enumerate() {
        by_content.entry(collapse(line)).or_default().push(index);
    }
    let mut votes: HashMap<isize, Vec<usize>> = HashMap::new();
    let mut considered = 0usize;
    for (offset, line) in old_string.lines().enumerate() {
        if !line.chars().any(char::is_alphanumeric) {
            continue;
        }
        considered += 1;
        for &index in by_content.get(&collapse(line)).into_iter().flatten() {
            votes
                .entry(index as isize - offset as isize)
                .or_default()
                .push(index);
        }
    }
    let (_, matched) = votes
        .into_iter()
        .max_by_key(|(start, lines)| (lines.len(), -start))?;
    if considered == 0 || matched.len() * 2 < considered {
        return None;
    }
    let first = *matched.iter().min()?;
    let last = *matched.iter().max()?;
    let shown = (last - first + 1).min(HINT_LINES);
    let mut hint = format!(
        "The closest stretch of the file (lines {}-{}) differs in whitespace or wording:",
        first + 1,
        first + shown
    );
    for (index, line) in file_lines.iter().enumerate().skip(first).take(shown) {
        let clipped: String = line.chars().take(HINT_LINE_CHARS).collect();
        hint.push_str(&format!("\n{:>6}\t{clipped}", index + 1));
    }
    Some(hint)
}
