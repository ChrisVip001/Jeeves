//! Rotating refresh tokens must have one consumer across the CLI, the resident worker and reconnects.
use crate::oauth::{discover, tokens_from_response, StoredAuth, TokenStore};
use reqwest::Url;
use std::{fs::File, time::Duration};

impl StoredAuth {
    pub fn matches_resource(&self, url: &str) -> bool {
        self.resource_url
            .as_deref()
            .is_none_or(|bound| Url::parse(url).is_ok_and(|parsed| parsed.as_str() == bound))
    }
}

impl TokenStore {
    pub(crate) fn lock(&self) -> Result<File, String> {
        let dir = self.path().parent().expect("auth file has parent");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.path().with_extension("lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|_| "concurrent OAuth credential update".to_string())?;
        Ok(file)
    }

    /// A live transport consults the current file, including after reauthorization or revocation.
    pub(crate) fn access(&self, url: &str) -> Result<String, String> {
        let auth = self.load();
        if !auth.matches_resource(url) {
            return Err("OAuth configuration changed; sign in again".into());
        }
        let tokens = auth.tokens.ok_or("OAuth authorization required")?;
        if tokens.expired() {
            return Err("OAuth token expired; sign in again".into());
        }
        Ok(tokens.access_token)
    }
}

/// True only when a rotation was persisted. Legacy files retain their on-connect flow.
pub async fn refresh(store: &TokenStore, url: &str, background: bool) -> Result<bool, String> {
    let auth = store.load();
    if !auth.tokens.as_ref().is_some_and(|t| t.expires_within(60)) {
        return Ok(false);
    }
    let _guard = store.lock()?;
    let mut auth = store.load();
    if !auth.matches_resource(url) {
        return Err("OAuth resource changed; sign in again".into());
    }
    let Some(tokens) = auth.tokens.as_ref().filter(|t| t.expires_within(60)) else {
        return Ok(false);
    };
    let refresh = tokens
        .refresh_token
        .as_deref()
        .ok_or("OAuth credential expires without a refresh token; sign in again")?;
    let client_id = auth
        .client_info
        .as_ref()
        .and_then(|v| v.get("client_id"))
        .and_then(|v| v.as_str())
        .ok_or("missing OAuth registration; sign in again")?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "could not prepare OAuth client")?;
    let endpoint = match (&auth.token_endpoint, &auth.resource_url) {
        (Some(endpoint), Some(_)) => endpoint.clone(),
        _ if background => {
            return Err("legacy OAuth credential; sign in to enable resident renewal".into())
        }
        _ => {
            discover(&client, url)
                .await
                .map_err(|_| "OAuth discovery unavailable")?
                .token_endpoint
        }
    };
    let endpoint = Url::parse(&endpoint).map_err(|_| "invalid token endpoint")?;
    let loopback = |u: &Url| matches!(u.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || !(endpoint.scheme() == "https"
            || (endpoint.scheme() == "http"
                && loopback(&endpoint)
                && Url::parse(url).is_ok_and(|u| u.scheme() == "http" && loopback(&u))))
    {
        return Err("insecure OAuth token endpoint".into());
    }
    let response = client
        .post(endpoint)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh),
        ])
        .send()
        .await
        .map_err(|_| "OAuth renewal service unavailable")?;
    let status = response.status();
    let reply = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "unreadable OAuth renewal response")?;
    if store.load() != auth {
        return Err("OAuth credentials changed during renewal".into());
    }
    if !status.is_success() {
        if matches!(status.as_u16(), 400 | 401) && reply["error"] == "invalid_grant" {
            auth.tokens = None;
            store.save(&auth)?;
            return Err("OAuth grant revoked; sign in again".into());
        }
        return Err(format!("OAuth renewal refused (HTTP {})", status.as_u16()));
    }
    let mut fresh = tokens_from_response(&reply)?;
    if fresh.refresh_token.is_none() {
        fresh.refresh_token = Some(refresh.to_owned());
    }
    auth.tokens = Some(fresh);
    store.save(&auth)?;
    Ok(true)
}
