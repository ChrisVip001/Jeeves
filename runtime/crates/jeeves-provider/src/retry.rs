//! Tiered backoff retries: **bounded, visible, never switching vendors**.
//!
//! Only two error classes are worth retrying (see `ProviderError::retryable`): rate
//! limiting (429) and transient server faults (5xx), plus network jitter and a stream that
//! breaks off before any text was shown (`stream_with_retries`). Retrying other
//! errors only fails again the same way — doubling the wait and burning one more quota.
//!
//! Three self-imposed constraints:
//!
//! * **Bounded**: at most 2 by default (`JEEVES_LLM_RETRIES`, cap 5), exponential backoff
//!   from 500ms, a per-wait cap of 8 seconds, and a total budget of 30 seconds per retry
//!   round. Seconds given by the server in `Retry-After` are honored but bound by these
//!   same caps — a server saying "come back in 600 seconds" must not hang a whole turn.
//! * **Visible**: every retry emits a notice (`RetryNotice`). Never retry silently —
//!   the user should see "rate limited, retrying", not a mysterious long pause.
//! * **Never switching vendors**: changing providers is another decision (a fallback
//!   chain to configure, a reason to explain); this only retries the same request once more.
//!
//! The notice outlet is **task-local** rather than a `Provider` trait parameter: the
//! `Provider` signature is a contract with loop, not worth touching for a notice; "this
//! turn" is exactly the task boundary (`run_turn` runs in its own task), so task-local is
//! naturally turn-scoped and cannot cross wires between two concurrent turns.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::ProviderError;

/// Starting point of the exponential backoff.
const BASE_DELAY: Duration = Duration::from_millis(500);
/// Per-wait cap (also bounds Retry-After).
const MAX_DELAY: Duration = Duration::from_secs(8);
/// Time budget for a whole retry round: past it, give up and let the error surface honestly.
const TOTAL_BUDGET: Duration = Duration::from_secs(30);

/// The content of one retry notice — enough for a human and for one UI line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryNotice {
    /// Which retry this is (starting from 1).
    pub attempt: u32,
    /// Why retrying (`HTTP 429` / `network error: ...`).
    pub reason: String,
    /// How long it waited.
    pub delay_ms: u64,
}

pub type NoteSink = Arc<dyn Fn(RetryNotice) + Send + Sync>;

tokio::task_local! {
    /// The notice outlet for this turn. Falls back to stderr without one (never silent).
    static NOTE_SINK: NoteSink;
}

/// Attach the notice outlet for this turn. `run_turn` uses it to wire retries to `EventSink`.
pub async fn with_note_sink<F: Future>(sink: NoteSink, future: F) -> F::Output {
    NOTE_SINK.scope(sink, future).await
}

/// Send a request, backing off and retrying as needed.
///
/// `send` must **rebuild** the request each time (reqwest's `RequestBuilder` consumes
/// itself), hence `FnMut` not a ready-made future. Returns the **unread-body** response.
pub async fn with_retries<F, Fut>(
    max_retries: u32,
    mut send: F,
) -> Result<reqwest::Response, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<reqwest::Response, ProviderError>>,
{
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        let error = match send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if status < 400 {
                    return Ok(response);
                }
                let retry_after = retry_after_secs(&response);
                let body = response.text().await.unwrap_or_default();
                ProviderError::from_http_retry(status, body, retry_after)
            }
            Err(error) => error,
        };

        match next_delay(&error, attempt, max_retries, started) {
            Some(delay) => {
                note_retry(&error, attempt + 1, delay);
                tokio::time::sleep(delay).await;
                attempt += 1;
            }
            None => return Err(error),
        }
    }
}

/// How long to wait next; `None` when retrying is pointless (or the budget is spent).
fn next_delay(
    error: &ProviderError,
    attempt: u32,
    max_retries: u32,
    started: Instant,
) -> Option<Duration> {
    if !error.retryable() || attempt >= max_retries {
        return None;
    }
    let backoff = (BASE_DELAY * 2u32.pow(attempt.min(4))).min(MAX_DELAY);
    let delay = error.retry_after().unwrap_or(backoff).min(MAX_DELAY);
    if started.elapsed() + delay > TOTAL_BUDGET {
        return None;
    }
    Some(delay)
}

/// The wait a response asks for, in whole seconds (rounded up): `retry-after-ms` first, then
/// `Retry-After` as seconds or as an HTTP date. A date in the past means "now".
fn retry_after_secs(response: &reqwest::Response) -> Option<u64> {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
    };
    parse_retry_after(
        header("retry-after-ms"),
        header(reqwest::header::RETRY_AFTER.as_str()),
        chrono::Utc::now(),
    )
}

pub(crate) fn parse_retry_after(
    millis: Option<&str>,
    value: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<u64> {
    if let Some(ms) = millis.and_then(|text| text.parse::<f64>().ok())
        && ms.is_finite()
        && ms >= 0.0
    {
        return Some((ms / 1000.0).ceil() as u64);
    }
    let value = value?;
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((at.with_timezone(&chrono::Utc) - now).num_seconds().max(0) as u64)
}

/// Run one streamed model call, starting it again when the stream breaks off **before any text
/// reached the user**. The HTTP-level retries inside `send` cover failures before the stream
/// starts; this covers a connection that opens and then dies. Once text has been shown, a retry
/// would print it twice, so the break is reported instead.
pub async fn stream_with_retries<F, Fut, T>(
    max_retries: u32,
    on_text: crate::TextSink,
    mut attempt_once: F,
) -> Result<T, ProviderError>
where
    F: FnMut(crate::TextSink) -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        let shown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let sink: crate::TextSink = {
            let shown = shown.clone();
            let on_text = on_text.clone();
            Arc::new(move |piece: &str| {
                shown.store(true, std::sync::atomic::Ordering::Release);
                on_text(piece);
            })
        };
        let error = match attempt_once(sink).await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let quiet = !shown.load(std::sync::atomic::Ordering::Acquire);
        let delay = matches!(error, ProviderError::Incomplete(_))
            .then(|| next_delay(&error, attempt, max_retries, started))
            .flatten()
            .filter(|_| quiet);
        let Some(delay) = delay else {
            return Err(error);
        };
        note_retry(&error, attempt + 1, delay);
        tokio::time::sleep(delay).await;
        attempt += 1;
    }
}

/// Say "retrying" out loud. This is a hard requirement: retries must never happen silently.
fn note_retry(error: &ProviderError, attempt: u32, delay: Duration) {
    let notice = RetryNotice {
        attempt,
        reason: reason_of(error),
        delay_ms: delay.as_millis() as u64,
    };
    if NOTE_SINK.try_with(|sink| sink(notice.clone())).is_err() {
        eprintln!(
            "(jeeves) model call failed ({}), retrying in {}ms (attempt {})",
            notice.reason, notice.delay_ms, notice.attempt
        );
    }
}

fn reason_of(error: &ProviderError) -> String {
    match error {
        ProviderError::Http { status, .. } => format!("HTTP {status}"),
        ProviderError::Network(e) => format!("network error: {e}"),
        ProviderError::Incomplete(why) => format!("stream ended early: {why}"),
        other => other.to_string(),
    }
}
