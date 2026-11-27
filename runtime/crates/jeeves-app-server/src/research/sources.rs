use super::{price, source_url};
use anyhow::{Result, ensure};
use std::{collections::BTreeMap, sync::LazyLock};

const MAX_REQUIRED_SOURCES: usize = 8;
static URLS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)https?://[^\s<>"'`（）【】。，、；！？]+"#)
        .expect("public research URL pattern")
});
static VERIFICATION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(concat!(
        r"(?i)(?:^|[.!?,:;。！？、，：；]\s*|\b(?:and|then)\s+)",
        r"(?:(?:please|can you|could you|help me)\s+)*(?:verify|confirm|validate|check (?:whether|if))\b",
        r"|(?:^|[。！？、，：；]\s*)(?:请帮我|请|帮我)?\s*(?:核实|核验|确认|查证)"
    ))
    .expect("explicit research verification pattern")
});

pub(super) fn requests_verification(question: &str) -> bool {
    VERIFICATION.is_match(&URLS.replace_all(question, " "))
}

pub(super) fn directive(line: &str) -> Option<&str> {
    line.strip_prefix("source ")
        .map(str::trim_start)
        .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
}

pub(super) fn required(
    source_lines: &[&str],
    questions: &[&str],
    prices: &BTreeMap<usize, price::Query>,
) -> Result<Vec<String>> {
    let mut urls = Vec::new();
    for raw in source_lines.iter().filter_map(|line| directive(line)) {
        push(&mut urls, source_url(raw)?);
    }
    for (index, question) in questions.iter().enumerate() {
        if let Some(price) = prices.get(&index) {
            if let Some(url) = price.feed_url() {
                push(&mut urls, source_url(url)?);
            }
            continue;
        }
        for found in URLS.find_iter(question) {
            let mut candidate = found.as_str().trim_end_matches(['.', ',', ';', '!', ':']);
            while candidate.ends_with(')')
                && candidate.matches(')').count() > candidate.matches('(').count()
            {
                candidate = &candidate[..candidate.len() - 1];
            }
            if let Ok(url) = source_url(candidate) {
                push(&mut urls, url);
            }
        }
    }
    ensure!(
        urls.len() <= MAX_REQUIRED_SOURCES,
        "Supply at most eight distinct required source URLs"
    );
    Ok(urls)
}

fn push(urls: &mut Vec<String>, url: String) {
    if !urls.contains(&url) {
        urls.push(url);
    }
}
