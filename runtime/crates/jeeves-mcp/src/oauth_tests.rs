//! Tests for oauth.
//!
//! The end-to-end one (sign_in) runs a **real local authorization server** and a real
//! callback listener — the only thing replaced is the browser: the test acts as the browser
//! and posts to the local callback port. Discover, register, PKCE, exchange, persist: all real.

use super::oauth::{
    StoredTokens, TokenStore, base64url_nopad, ensure_access_token, pkce_challenge, pkce_verifier,
    sanitise, sign_in,
};
use serde_json::json;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn server_names_become_safe_filenames() {
    assert_eq!(sanitise("team-memory"), "team-memory");
    assert_eq!(sanitise("../evil"), "___evil");
    assert_eq!(sanitise("a b/c"), "a_b_c");
    assert_eq!(sanitise("a very long name").len(), "a very long name".len());
}

#[test]
fn token_store_round_trips_and_stays_private() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = TokenStore::new(dir.path(), "demo");

    store
        .save_client_info(json!({"client_id": "cid", "secret": "nope"}))
        .expect("store client_info");
    store
        .save_tokens(StoredTokens {
            access_token: "at".to_string(),
            refresh_token: Some("rt".to_string()),
            expires_at: Some(i64::MAX),
        })
        .expect("store the tokens");

    let loaded = store.load();
    assert_eq!(loaded.client_info.as_ref().unwrap()["client_id"], "cid");
    let tokens = loaded.tokens.expect("tokens");
    assert_eq!(tokens.access_token, "at");
    assert!(!tokens.expired());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the credential file has to be 0600");
    }
}

#[test]
fn a_corrupt_token_file_reads_as_absent() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = TokenStore::new(dir.path(), "broken");
    std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
    std::fs::write(store.path(), "this is not JSON").unwrap();
    assert!(
        store.load().tokens.is_none(),
        "a bad file must not be fatal"
    );
    assert!(store.tokens().is_none());
}

#[test]
fn expired_is_judged_from_the_clock() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(
        !StoredTokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(now + 3600)
        }
        .expired()
    );
    assert!(
        StoredTokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: Some(now - 10)
        }
        .expired()
    );
    assert!(
        !StoredTokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: None
        }
        .expired()
    );
}

#[test]
fn pkce_matches_the_rfc_vector() {
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(
        pkce_challenge(verifier),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn base64url_avoids_the_url_hostile_alphabet() {
    let encoded = base64url_nopad(&[0xfb, 0xff, 0xbf, 0xff]);
    assert!(
        !encoded.contains('+') && !encoded.contains('/') && !encoded.contains('='),
        "{encoded}"
    );
    assert_eq!(base64url_nopad(&[0x00]).len(), 2);
    assert_eq!(base64url_nopad(&[0x00, 0x00]).len(), 3);
    assert_eq!(base64url_nopad(&[0x00, 0x00, 0x00]).len(), 4);
    assert_eq!(
        pkce_verifier().len(),
        64,
        "48 bytes of base64url is exactly 64 characters"
    );
}

#[tokio::test]
async fn sign_in_walks_the_whole_flow_against_a_local_authorization_server() {
    let received =
        std::sync::Arc::new(tokio::sync::Mutex::new(Vec::<Vec<(String, String)>>::new()));
    let received_form = received.clone();

    let auth = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let auth_port = auth.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        loop {
            let (mut socket, _) = auth.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            let target = request_line_target(&request);
            let response = if target.contains("/.well-known/oauth-protected-resource") {
                http_json(
                    200,
                    &json!({"authorization_servers": [format!("http://127.0.0.1:{auth_port}")]}),
                )
            } else if target.contains("/.well-known/oauth-authorization-server") {
                http_json(
                    200,
                    &json!({
                        "authorization_endpoint": format!("http://127.0.0.1:{auth_port}/authorize"),
                        "token_endpoint": format!("http://127.0.0.1:{auth_port}/token"),
                        "registration_endpoint": format!("http://127.0.0.1:{auth_port}/register"),
                    }),
                )
            } else if target.contains("/register") {
                http_json(200, &json!({"client_id": "client-123"}))
            } else if target.contains("/token") {
                let form = parse_form(&request);
                received_form.lock().await.push(form.clone());
                assert!(
                    form.iter()
                        .any(|(k, v)| k == "code_verifier" && !v.is_empty())
                );
                assert!(form.iter().any(|(k, v)| k == "code" && v == "the-code"));
                http_json(
                    200,
                    &json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3600}),
                )
            } else {
                http_json(404, &json!({"no": "pe"}))
            };
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });

    let server_url = format!("http://127.0.0.1:{auth_port}/mcp");
    let dir = tempfile::tempdir().unwrap();

    let authorize_slot: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let browser = {
        let authorize_slot = authorize_slot.clone();
        tokio::spawn(async move {
            let authorize_url = loop {
                if let Some(url) = authorize_slot.lock().unwrap().clone() {
                    break url;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            let state = authorize_url
                .split('&')
                .find(|p| p.starts_with("state="))
                .map(|p| p.trim_start_matches("state=").to_string())
                .expect("the authorisation URL carries a state");

            let mut stream = None;
            for _ in 0..100 {
                match tokio::net::TcpStream::connect("127.0.0.1:41765").await {
                    Ok(found) => {
                        stream = Some(found);
                        break;
                    }
                    Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
                }
            }
            let mut stream = stream.expect("the callback port did not come up");
            stream
                .write_all(
                    format!(
                    "GET /callback?code=the-code&state={state} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n"
                )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            String::from_utf8_lossy_owned(response)
        })
    };

    let client = reqwest::Client::new();
    let token = sign_in(&client, dir.path(), "demo", &server_url, &|url| {
        assert!(url.contains("/authorize?"));
        assert!(url.contains("code_challenge_method=S256"));
        *authorize_slot.lock().unwrap() = Some(url.to_string());
    })
    .await
    .expect("the whole sign-in flow has to work");

    assert_eq!(token, "at-1");
    browser.await.unwrap();

    let store = TokenStore::new(dir.path(), "demo");
    let loaded = store.load();
    assert!(loaded.matches_resource(&server_url));
    assert!(loaded.resource_url.is_some() && loaded.token_endpoint.is_some());
    assert_eq!(
        loaded.tokens.unwrap().refresh_token.as_deref(),
        Some("rt-1")
    );
    assert_eq!(loaded.client_info.unwrap()["client_id"], "client-123");

    let forms = received.lock().await.clone();
    let token_request = forms.last().expect("the token request arrives");
    assert!(
        token_request
            .iter()
            .any(|(k, v)| k == "client_id" && v == "client-123")
    );
    assert!(
        token_request
            .iter()
            .any(|(k, v)| k == "grant_type" && v == "authorization_code")
    );

    let error = tokio::time::timeout(
        Duration::from_secs(5),
        sign_in(&client, dir.path(), "demo", &server_url, &|_| {
            tokio::spawn(async {
                let mut stream = tokio::net::TcpStream::connect("127.0.0.1:41765")
                    .await
                    .unwrap();
                stream
                    .write_all(
                        b"GET /callback?code=fixture-code HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
                    )
                    .await
                    .unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).await.unwrap();
            });
        }),
    )
    .await
    .expect("fixture callback ended")
    .expect_err("missing state must be rejected");
    assert!(
        error.contains("state is missing or does not match"),
        "{error}"
    );
    assert_eq!(received.lock().await.len(), 1);
    assert_eq!(store.tokens().unwrap().access_token, "at-1");

    server.abort();
}

#[tokio::test]
async fn ensure_token_without_a_login_points_at_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let error = ensure_access_token(dir.path(), "ghost", "http://127.0.0.1:1/mcp")
        .await
        .expect_err("never signed in must error");
    assert!(error.contains("jeeves mcp login ghost"), "{error}");
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(head_end) = find_end(&buffer) {
            let head = String::from_utf8_lossy_owned(buffer[..head_end].to_vec());
            let length: usize = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            let body_have = buffer.len() - head_end - 4;
            if body_have >= length {
                break;
            }
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
        }
    }
    String::from_utf8_lossy_owned(buffer)
}

fn find_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|w| w == b"\r\n\r\n")
}

fn request_line_target(request: &str) -> String {
    request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string()
}

fn parse_form(request: &str) -> Vec<(String, String)> {
    let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
    body.trim()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect()
}

fn http_json(status: u16, body: &serde_json::Value) -> String {
    let raw = body.to_string();
    format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{raw}",
        raw.len()
    )
}
