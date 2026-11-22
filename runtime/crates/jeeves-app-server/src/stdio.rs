//! The stdio transport: one JSON-RPC message per line.
//!
//! Newline-delimited JSON was chosen over Content-Length framing because it can be hand-debugged with
//! `echo '…' | jeeves app-server`, and connected from any language in three lines of code
//! (write a line, read a line). The complexity of LSP/MCP-style framing buys nothing comparable
//! at this scale.
//!
//! **stdout has exactly one writer.** The request loop pushes notifications and responses into the
//! channel, and the writer persists them in order, flushing frame by frame — so the streamed
//! `textDelta`s arrive at the browser one by one, without interleaving with responses or corrupting lines.

use anyhow::Result;
use jeeves_protocol::{
    codes, ErrorObject, JsonRpcError, JsonRpcMessage, JsonRpcNotification, JsonRpcRequest,
    JSONRPC_VERSION,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::{handle, EventSink, Frame, Server};

pub async fn run_stdio(server: Server) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();

    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(frame) = rx.recv().await {
            let text = match frame {
                Frame::Notification(notification) => {
                    let envelope = JsonRpcNotification {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        method: "turn/notification".to_string(),
                        params: Some(serde_json::to_value(&notification).expect("serialize")),
                    };
                    serde_json::to_string(&envelope).expect("serialize")
                }
                Frame::Response(response) => serde_json::to_string(&response).expect("serialize"),
            };
            if stdout.write_all(text.as_bytes()).await.is_err()
                || stdout.write_all(b"\n").await.is_err()
                || stdout.flush().await.is_err()
            {
                break;
            }
        }
    });

    {
        let server = server.clone();
        let sink = EventSink::new(tx.clone());
        tokio::spawn(async move {
            crate::resume::reconcile(&server, &sink).await;
            crate::task::dispatch::kick(server, sink);
        });
    }

    let renewal = tokio::spawn(crate::mcp_renewal::run(server.clone()));
    let weekly_host = server.clone();
    let weekly_sink = EventSink::new(tx.clone());
    let weekly = tokio::spawn(async move {
        if let Err(error) = crate::responsibility::worker::run(weekly_host, weekly_sink).await {
            eprintln!("(jeeves) weekly report worker stopped: {error}");
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let sink = EventSink::new(tx.clone());
        match serde_json::from_str::<JsonRpcRequest>(line) {
            Ok(request) => {
                if runs_concurrently(&request.method) {
                    let server = server.clone();
                    tokio::spawn(async move {
                        handle(&server, request, sink).await;
                    });
                } else {
                    handle(&server, request, sink).await;
                }
            }
            Err(e) => sink.response(JsonRpcMessage::Error(JsonRpcError {
                jsonrpc: JSONRPC_VERSION.to_string(),
                id: None,
                error: ErrorObject {
                    code: codes::PARSE_ERROR,
                    message: format!("parse failed: {e}"),
                    data: None,
                },
            })),
        }
    }

    renewal.abort();
    let _ = renewal.await;
    weekly.abort();
    let _ = weekly.await;
    drop(tx);
    let _ = writer.await;
    server.stop_memory_worker().await;
    server.mcp.close().await;
    Ok(())
}

/// Methods that run a whole turn and so are spawned instead of awaited in the read loop.
/// `interaction/start` runs a turn just like `turn/start` (and holds its own per-session lock);
/// awaiting it inline blocked the `approval/respond` a chat gateway sends while the turn waits.
fn runs_concurrently(method: &str) -> bool {
    use jeeves_protocol::methods::*;
    method == TURN_START
        || method == INTERACTION_START
        || matches!(
            method,
            BROWSER_INPUT
                | BROWSER_DOWNLOAD
                | BROWSER_DOWNLOADS
                | BROWSER_START
                | BROWSER_FRAME
                | BROWSER_CONTROL
                | DESKTOP_SELECT
                | DESKTOP_OBSERVE
                | DESKTOP_ACT
                | DESKTOP_TAKEOVER
                | DESKTOP_MODEL
                | DESKTOP_FRAME
                | DESKTOP_SHARE
        )
}

#[cfg(test)]
mod tests {
    use super::runs_concurrently;
    use jeeves_protocol::methods;

    #[test]
    fn turns_are_spawned_so_approvals_can_still_get_in() {
        assert!(runs_concurrently(methods::TURN_START));
        assert!(runs_concurrently(methods::INTERACTION_START));
        for method in [
            methods::DESKTOP_SELECT,
            methods::DESKTOP_ACT,
            methods::DESKTOP_SHARE,
        ] {
            assert!(runs_concurrently(method));
        }
        assert!(!runs_concurrently(methods::APPROVAL_RESPOND));
        assert!(!runs_concurrently(methods::MEMORY_REMEMBER));
    }
}
