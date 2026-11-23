//! Parse bounded result entries, keeping titles, source links and snippets together.
use super::{strip_html, Hit};
use base64::{engine::general_purpose, Engine as _};

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    for (at, _) in tag.match_indices(name) {
        if at > 0 && !tag.as_bytes()[at - 1].is_ascii_whitespace() {
            continue;
        }
        let Some(value) = tag[at + name.len()..].trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let quote = value.chars().next()?;
        if !matches!(quote, '\'' | '"') {
            continue;
        }
        let value = &value[1..];
        return Some(&value[..value.find(quote)?]);
    }
    None
}

fn has_class(tag: &str, class: &str) -> bool {
    attr(tag, "class").is_some_and(|value| value.split_whitespace().any(|item| item == class))
}

fn links<'a>(page: &'a str) -> impl Iterator<Item = (usize, usize, &'a str, &'a str)> + 'a {
    page.match_indices("<a").filter_map(|(start, _)| {
        let tag = &page[start..];
        if !tag.as_bytes().get(2)?.is_ascii_whitespace() {
            return None;
        }
        let open = tag.find('>')? + 1;
        let close = tag[open..].find("</a>")? + open;
        Some((start, start + close + 4, &tag[..open], &tag[open..close]))
    })
}

fn source_url(href: &str, base: &str) -> Option<String> {
    let href = href.replace("&amp;", "&").replace("&#38;", "&");
    let mut url = reqwest::Url::parse(&href)
        .or_else(|_| reqwest::Url::parse(base)?.join(&href))
        .ok()?;
    let host = url.host_str()?;
    if (host == "duckduckgo.com" || host.ends_with(".duckduckgo.com")) && url.path() == "/l/" {
        let target = url
            .query_pairs()
            .find(|(key, _)| key == "uddg")?
            .1
            .into_owned();
        url = reqwest::Url::parse(&target).ok()?;
    } else if (host == "bing.com" || host.ends_with(".bing.com")) && url.path() == "/ck/a" {
        let encoded = url
            .query_pairs()
            .find(|(key, _)| key == "u")?
            .1
            .into_owned();
        let encoded = encoded.strip_prefix("a1")?.trim_end_matches('=');
        let bytes = general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded))
            .ok()?;
        url = reqwest::Url::parse(std::str::from_utf8(&bytes).ok()?).ok()?;
    }
    if !matches!(url.scheme(), "https" | "http") || url.host_str().is_none() {
        return None;
    }
    Some(url.into())
}

fn snippet(block: &str, marker: &str, close: &str) -> Option<String> {
    let at = block.find(marker)?;
    let open = block[at..].find('>')? + at + 1;
    let end = block[open..].find(close)? + open;
    Some(strip_html(&block[open..end]))
}

pub(crate) fn parse_duckduckgo(page: &str, max_results: u32) -> Vec<Hit> {
    let entries: Vec<_> = links(page)
        .filter(|(_, _, tag, _)| has_class(tag, "result-link") || has_class(tag, "result__a"))
        .collect();
    let hits = entries
        .iter()
        .enumerate()
        .filter_map(|(index, (_, end, tag, title))| {
            let url = source_url(attr(tag, "href")?, "https://duckduckgo.com/")?;
            let next = entries
                .get(index + 1)
                .map(|entry| entry.0)
                .unwrap_or(page.len());
            let block = &page[*end..next];
            let text = snippet(block, "result-snippet", "</td>")
                .or_else(|| snippet(block, "result__snippet", "</a>"))
                .unwrap_or_default();
            Some((strip_html(title), text, url))
        })
        .collect();
    super::clean_hits(hits, max_results)
}

pub(crate) fn parse_bing(page: &str, max_results: u32) -> Vec<Hit> {
    let hits = page
        .split("<li")
        .filter_map(|block| {
            let open = block.find('>')? + 1;
            if !has_class(&block[..open], "b_algo") {
                return None;
            }
            let block = block.split("</li>").next()?;
            let heading = block.find("<h2")?;
            let heading_end = block[heading..].find("</h2>")? + heading;
            let (_, _, tag, title) = links(&block[heading..heading_end]).next()?;
            let url = source_url(attr(tag, "href")?, "https://www.bing.com/")?;
            let text = snippet(&block[heading_end..], "<p", "</p>").unwrap_or_default();
            Some((strip_html(title), text, url))
        })
        .collect();
    super::clean_hits(hits, max_results)
}
