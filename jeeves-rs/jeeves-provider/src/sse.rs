//! Minimal SSE parsing: only `data:` lines matter.
//!
//! Anthropic and OpenAI streaming answers share the same envelope:
//!
//! ```text
//! event: content_block_delta
//! data: {"type":"content_block_delta",...}
//! (blank line)
//! ```
//!
//! Event names and structures differ completely, but "one data: payload per line" is the
//! same — parsing stops at this layer; the rest belongs to each implementation.

use bytes::Bytes;
use futures_util::{Stream, StreamExt};

use crate::ProviderError;

/// Turn the HTTP byte stream into one `data:` payload per item.
///
/// `event:` lines, comments, and blank lines are dropped — event boundaries mean nothing
/// to the caller, which wants JSON items one by one. OpenAI's `data: [DONE]` is passed
/// through verbatim too; the caller decides.
pub fn data_lines<S>(bytes: S) -> impl Stream<Item = Result<String, ProviderError>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    futures_util::stream::unfold(
        (bytes, bytes::BytesMut::new(), 0, false),
        |(mut stream, mut buffer, mut scanned, mut eof)| async move {
            loop {
                let line = if let Some(offset) = buffer[scanned..].iter().position(|b| *b == b'\n')
                {
                    buffer.split_to(scanned + offset + 1)
                } else if eof {
                    if buffer.is_empty() {
                        return None;
                    }
                    buffer.split()
                } else {
                    scanned = buffer.len();
                    match stream.next().await {
                        Some(Ok(chunk)) => buffer.extend_from_slice(&chunk),
                        Some(Err(e)) => {
                            return Some((
                                Err(ProviderError::Network(e)),
                                (stream, bytes::BytesMut::new(), 0, true),
                            ))
                        }
                        None => eof = true,
                    }
                    continue;
                };
                scanned = 0;
                if let Some(data) = line.strip_prefix(b"data:") {
                    let payload = std::str::from_utf8(data)
                        .map(|text| text.trim().to_owned())
                        .map_err(|_| ProviderError::Parse("SSE data is not UTF-8".into()));
                    if !matches!(&payload, Ok(text) if text.is_empty()) {
                        return Some((payload, (stream, buffer, scanned, eof)));
                    }
                }
            }
        },
    )
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod sse_tests;
