//! Tests for SSE parsing. No network — constructed byte streams are fed directly.

use bytes::Bytes;
use futures_util::{stream, StreamExt};

use super::data_lines;

fn bytes_stream(
    chunks: Vec<&str>,
) -> impl futures_util::Stream<Item = Result<Bytes, reqwest::Error>> {
    stream::iter(
        chunks
            .into_iter()
            .map(|c| Ok(Bytes::from(c.to_string())))
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn yields_each_data_payload() {
    let out = data_lines(bytes_stream(vec![
        "event: message_start\n",
        "data: {\"type\":\"message_start\"}\n",
        "\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\"}\n",
        "\n",
    ]))
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .map(|r| r.expect("it should not error"))
    .collect::<Vec<_>>();

    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(out[0], "{\"type\":\"message_start\"}");
    assert_eq!(out[1], "{\"type\":\"content_block_delta\"}");
}

#[tokio::test]
async fn handles_payloads_split_across_chunks() {
    let out = data_lines(bytes_stream(vec![
        "event: x\ndata: {\"a\":",
        "1}\ndata: {\"b\":2}\n\ndata: [DONE]",
        "\n",
    ]))
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .map(|r| r.expect("it should not error"))
    .collect::<Vec<_>>();

    assert_eq!(
        out,
        vec![
            "{\"a\":1}".to_string(),
            "{\"b\":2}".to_string(),
            "[DONE]".to_string()
        ]
    );
}

#[tokio::test]
async fn handles_crlf_and_missing_trailing_blank_line() {
    let out = data_lines(bytes_stream(vec![
        "data: {\"a\":1}\r\n\r\ndata: {\"b\":2}\r\n",
    ]))
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .map(|r| r.expect("it should not error"))
    .collect::<Vec<_>>();

    assert_eq!(out, vec!["{\"a\":1}".to_string(), "{\"b\":2}".to_string()]);
}

#[tokio::test]
async fn ignores_comments_and_event_lines() {
    let out = data_lines(bytes_stream(vec![
        ": keep-alive comment\n",
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        ": another\n",
    ]))
    .collect::<Vec<_>>()
    .await
    .into_iter()
    .map(|r| r.expect("it should not error"))
    .collect::<Vec<_>>();

    assert_eq!(out, vec!["{\"type\":\"ping\"}".to_string()]);
}

#[tokio::test]
async fn review_sse_preserves_split_codepoints_and_ignores_empty_final_data() {
    let bytes = "data: 中文 🦀\r\n\ndata:   ".as_bytes();
    let chunks = stream::iter(bytes.iter().map(|b| Ok(Bytes::copy_from_slice(&[*b]))));
    let out = data_lines(chunks).collect::<Vec<_>>().await;
    assert_eq!(
        out.iter()
            .map(|r| r.as_ref().unwrap().as_str())
            .collect::<Vec<_>>(),
        ["中文 🦀"]
    );
}
