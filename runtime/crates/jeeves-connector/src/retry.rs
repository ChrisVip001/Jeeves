//! Bounded, visible retries for the connector's **idempotent** HTTP calls.
//!
//! Only network errors, HTTP 429 and 5xx are worth retrying; other 4xx and business-code
//! refusals are real answers and must surface at once. Non-idempotent calls (sending a
//! message, creating a job) are deliberately NOT routed through here: a retry could
//! duplicate a side effect the person only approved once.
//!
//! Three limits, mirroring the provider's retry policy: at most `MAX_RETRIES` attempts,
//! exponential backoff from 500ms capped at 8s, and `Retry-After` honoured only within
//! that cap. Every retry writes one line to stderr — a silent pause looks like a hang.

use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};

const MAX_RETRIES: u32 = 2;
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_DELAY: Duration = Duration::from_secs(8);

fn retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    let exponential = BASE_DELAY * 2u32.pow(attempt.min(4));
    retry_after.unwrap_or(exponential).min(MAX_DELAY)
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

/// Send an idempotent request with bounded exponential backoff. `send` rebuilds the
/// request on every attempt (a reqwest `RequestBuilder` consumes itself).
pub async fn with_retries<F, Fut>(mut send: F) -> Result<reqwest::Response>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response>>,
{
    let mut attempt = 0u32;
    loop {
        match send().await {
            Ok(response) if !retryable_status(response.status()) => return Ok(response),
            Ok(response) => {
                let status = response.status();
                let delay = backoff(attempt, retry_after(&response));
                if attempt >= MAX_RETRIES {
                    return Ok(response);
                }
                eprintln!(
                    "(connector) HTTP {status}, retrying in {}ms",
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }
            Err(error) => {
                if attempt >= MAX_RETRIES {
                    return Err(error).context("connector request failed after retries");
                }
                let delay = backoff(attempt, None);
                eprintln!(
                    "(connector) network error ({error}), retrying in {}ms",
                    delay.as_millis()
                );
                tokio::time::sleep(delay).await;
            }
        }
        attempt += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_429_and_5xx_are_retryable() {
        assert!(retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(retryable_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR));
        assert!(retryable_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!retryable_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!retryable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!retryable_status(reqwest::StatusCode::OK));
    }

    #[test]
    fn backoff_is_bounded_even_when_server_asks_for_a_long_wait() {
        assert_eq!(backoff(0, None), Duration::from_millis(500));
        assert_eq!(backoff(1, None), Duration::from_millis(1000));
        assert_eq!(backoff(9, None), Duration::from_secs(8));
        assert_eq!(
            backoff(0, Some(Duration::from_secs(600))),
            Duration::from_secs(8)
        );
    }

    async fn serve_failures_then(statuses: &[u16]) -> (String, tokio::task::JoinHandle<()>) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let statuses = statuses.to_vec();
        let handle = tokio::spawn(async move {
            for status in statuses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                seen.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf).await;
                let body = "{}";
                let reason = if status == 429 {
                    "Too Many Requests"
                } else {
                    "OK"
                };
                let reply = format!(
                    "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    #[tokio::test]
    async fn a_transient_429_then_success_retries_once() {
        let (base, server) = serve_failures_then(&[429, 200]).await;
        let attempts = std::cell::Cell::new(0u32);
        let response = with_retries(|| {
            let n = attempts.get();
            attempts.set(n + 1);
            async { reqwest::get(format!("{base}/")).await.map_err(Into::into) }
        })
        .await
        .expect("the second attempt succeeds");
        assert_eq!(response.status().as_u16(), 200);
        server.await.ok();
    }

    #[tokio::test]
    async fn a_real_4xx_is_not_retried() {
        let (base, server) = serve_failures_then(&[401]).await;
        let attempts = std::cell::Cell::new(0u32);
        let response = with_retries(|| {
            let n = attempts.get();
            attempts.set(n + 1);
            async { reqwest::get(format!("{base}/")).await.map_err(Into::into) }
        })
        .await
        .expect("the error response itself is not an Err");
        assert_eq!(response.status().as_u16(), 401);
        assert_eq!(attempts.get(), 1, "a real 4xx must surface at once");
        server.await.ok();
    }
}
