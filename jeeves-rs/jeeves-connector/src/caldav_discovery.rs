//! RFC 5397 / 4791 read-only discovery from a configured CalDAV service root.
//! Never forward credentials outside the explicitly configured origin (for the iCloud
//! root, also allow Apple's numbered CalDAV shard hosts). No XML entity expansion.
use anyhow::{bail, Context, Result};
use reqwest::{Client, Url};

const MAX_XML: usize = 2 * 1024 * 1024;
const MAX_CALENDARS: usize = 8;

fn trusted(root: &Url, next: &Url) -> bool {
    if !next.username().is_empty()
        || next.password().is_some()
        || next.query().is_some()
        || next.fragment().is_some()
    {
        return false;
    }
    let https = next.scheme() == "https";
    let fixture = cfg!(test) && next.scheme() == "http" && next.host_str() == Some("127.0.0.1");
    if !https && !fixture {
        return false;
    }
    if root.origin() == next.origin() {
        return true;
    }
    root.scheme() == "https"
        && root.host_str() == Some("caldav.icloud.com")
        && next.port_or_known_default() == Some(443)
        && next
            .host_str()
            .and_then(|h| h.strip_prefix('p'))
            .and_then(|h| h.strip_suffix("-caldav.icloud.com"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn resolve(root: &Url, base: &Url, href: &str) -> Result<Url> {
    let next = base
        .join(href.trim())
        .context("invalid CalDAV discovery href")?;
    anyhow::ensure!(
        trusted(root, &next),
        "CalDAV discovery refused to send credentials outside the configured service"
    );
    Ok(next)
}

pub(crate) fn elements<'a>(xml: &'a str, want: &str) -> Result<Vec<&'a str>> {
    if xml.to_ascii_lowercase().contains("<!doctype") {
        bail!("CalDAV discovery refused a DOCTYPE");
    }
    let (mut cursor, mut depth, mut start) = (0, 0usize, 0);
    let mut out = Vec::new();
    while let Some(at) = xml[cursor..].find('<') {
        let open = cursor + at;
        if let Some((prefix, end)) = [("<!--", "-->"), ("<![CDATA[", "]]>")]
            .into_iter()
            .find(|(p, _)| xml[open..].starts_with(p))
        {
            cursor = open
                + prefix.len()
                + xml[open + prefix.len()..]
                    .find(end)
                    .context("unterminated XML section")?
                + end.len();
            continue;
        }
        let close = open
            + xml[open..]
                .find('>')
                .context("unterminated CalDAV XML tag")?;
        let raw = xml[open + 1..close].trim();
        cursor = close + 1;
        if raw.starts_with(['!', '?']) {
            continue;
        }
        let closing = raw.starts_with('/');
        let empty = raw.ends_with('/');
        let name = raw
            .trim_start_matches('/')
            .trim_end_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("")
            .rsplit(':')
            .next()
            .unwrap_or("");
        if name != want {
            continue;
        }
        if closing {
            anyhow::ensure!(depth > 0, "unexpected CalDAV XML close");
            depth -= 1;
            if depth == 0 {
                out.push(&xml[start..open]);
            }
        } else if empty {
            if depth == 0 {
                out.push("");
            }
        } else {
            if depth == 0 {
                start = close + 1;
            }
            depth += 1;
        }
    }
    anyhow::ensure!(depth == 0, "unterminated CalDAV XML element");
    Ok(out)
}

fn href(xml: &str) -> Result<String> {
    let raw = elements(xml, "href")?
        .into_iter()
        .next()
        .context("CalDAV discovery did not return an href")?
        .trim();
    anyhow::ensure!(!raw.is_empty(), "invalid CalDAV href");
    let raw = raw.replace("&amp;", "&");
    anyhow::ensure!(!raw.contains(['<', '>', '\r', '\n']), "invalid CalDAV href");
    Ok(raw)
}

async fn propfind(
    client: &Client,
    root: &Url,
    url: Url,
    depth: &str,
    properties: &str,
    user: &str,
    secret: &str,
) -> Result<(Url, String)> {
    let mut current = url;
    let request = format!(
        r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop>{properties}</d:prop></d:propfind>"#
    );
    for _ in 0..4 {
        let resp = client
            .request(reqwest::Method::from_bytes(b"PROPFIND")?, current.clone())
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .basic_auth(user, Some(secret))
            .body(request.clone())
            .send()
            .await
            .context("CalDAV discovery request failed")?;
        if resp.status().is_redirection() {
            let location = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .context("CalDAV redirect missing Location")?
                .to_str()?;
            current = resolve(root, &current, location)?;
            continue;
        }
        anyhow::ensure!(
            resp.status().is_success(),
            "CalDAV discovery returned HTTP {}",
            resp.status()
        );
        use futures_util::StreamExt;
        let mut stream = resp.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            anyhow::ensure!(
                bytes.len() + chunk.len() <= MAX_XML,
                "CalDAV discovery response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        return Ok((current, String::from_utf8(bytes)?));
    }
    bail!("too many CalDAV discovery redirects")
}

fn collections(root: &Url, base: &Url, xml: &str) -> Result<Vec<Url>> {
    let mut urls = Vec::new();
    for response in elements(xml, "response")? {
        let mut calendar = false;
        for kind in elements(response, "resourcetype")? {
            calendar |= !elements(kind, "calendar")?.is_empty();
        }
        if !calendar {
            continue;
        }
        let url = resolve(root, base, &href(response)?)?;
        if !urls.contains(&url) {
            urls.push(url);
        }
        anyhow::ensure!(urls.len()<=MAX_CALENDARS,"CalDAV account has more than eight collections; configure the desired collection URL explicitly");
    }
    anyhow::ensure!(
        !urls.is_empty(),
        "CalDAV discovery found no calendar collections"
    );
    Ok(urls)
}

pub(crate) async fn discover(
    client: &Client,
    root: &Url,
    user: &str,
    secret: &str,
) -> Result<Vec<Url>> {
    anyhow::ensure!(trusted(root, root), "untrusted CalDAV discovery target");
    let (base, body) = propfind(
        client,
        root,
        root.clone(),
        "0",
        "<d:current-user-principal/>",
        user,
        secret,
    )
    .await?;
    let principal = elements(&body, "current-user-principal")?
        .into_iter()
        .next()
        .context("CalDAV principal not found")?;
    let principal = resolve(root, &base, &href(principal)?)?;
    let (base, body) = propfind(
        client,
        root,
        principal,
        "0",
        "<c:calendar-home-set/>",
        user,
        secret,
    )
    .await?;
    let home = elements(&body, "calendar-home-set")?
        .into_iter()
        .next()
        .context("CalDAV calendar home not found")?;
    let home = resolve(root, &base, &href(home)?)?;
    let (base, body) = propfind(client, root, home, "1", "<d:resourcetype/>", user, secret).await?;
    collections(root, &base, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_service_origin_or_icloud_caldav_shards_are_trusted() {
        let root = Url::parse("https://caldav.icloud.com/").unwrap();
        assert!(resolve(&root, &root, "https://p12-caldav.icloud.com/cal/").is_ok());
        for url in [
            "https://caldav.icloud.com.example.com/",
            "https://p12-caldav.icloud.com:444/",
            "http://p12-caldav.icloud.com/",
            "https://user:secret@caldav.icloud.com/",
            "https://example.test/",
        ] {
            assert!(resolve(&root, &root, url).is_err(), "{url}");
        }
        let root = Url::parse("https://dav.example.test/").unwrap();
        assert!(resolve(&root, &root, "https://other.example.test/").is_err());
    }
    #[test]
    fn only_calendar_collection_responses_are_used() {
        let root = Url::parse("https://dav.example.test/").unwrap();
        let xml = r#"<d:multistatus><d:response><d:href>/home/</d:href><d:resourcetype><d:collection/></d:resourcetype></d:response><d:response><d:href>/home/one/</d:href><d:resourcetype><c:calendar/></d:resourcetype></d:response></d:multistatus>"#;
        assert_eq!(
            collections(&root, &root, xml).unwrap()[0].path(),
            "/home/one/"
        );
        assert!(elements("<!DOCTYPE x SYSTEM 'file:///tmp/secret'>", "href").is_err());
    }

    #[test]
    fn malformed_collection_is_not_silently_omitted() {
        let root = Url::parse("https://dav.example.test/").unwrap();
        let xml = r#"<d:multistatus><d:response><d:href>/home/one/</d:href><d:resourcetype><c:calendar/></d:resourcetype></d:response><d:response><d:href>/home/two/</d:href><d:resourcetype><c:calendar></d:resourcetype></d:response></d:multistatus>"#;
        assert!(collections(&root, &root, xml).is_err());
    }
}
