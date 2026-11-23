use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use axum::Router;
use axum::http::{HeaderValue, StatusCode};
use axum::routing::head;
use ring::hmac;

pub(super) fn publish(router: Router, home: &Path, port: u16, token: &str) -> Result<Router> {
    let mut instance = [0; 16];
    getrandom::fill(&mut instance)
        .map_err(|error| anyhow::anyhow!("generating a Runtime instance ID: {error}"))?;
    let instance = hex(&instance);
    let key = hmac::Key::new(hmac::HMAC_SHA256, token.as_bytes());
    let proof = HeaderValue::from_str(&hex(hmac::sign(&key, instance.as_bytes()).as_ref()))?;

    let mut publication = tempfile::NamedTempFile::new_in(home)?;
    serde_json::to_writer(
        publication.as_file_mut(),
        &serde_json::json!({ "port": port, "instance": instance }),
    )?;
    publication.flush()?;
    publication
        .persist(home.join("runtime-endpoint.json"))
        .context("publishing the loopback Runtime endpoint")?;
    Ok(router.route(
        "/internal/runtime-ready",
        head(move || {
            let proof = proof.clone();
            async move { (StatusCode::NO_CONTENT, [("x-jeeves-runtime-proof", proof)]) }
        }),
    ))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}
