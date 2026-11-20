//! Host-injected search service; local implementations remain the fallback.

use crate::{Handler, ToolCtx};
use serde_json::Value;

pub(super) async fn call(primary: &Option<Handler>, ctx: ToolCtx, args: Value) -> Option<String> {
    let handler = primary.as_ref()?;
    match handler(ctx, args).await {
        Ok(text) if !text.trim().is_empty() && !text.trim_start().starts_with("Error:") => {
            Some(super::finish_fetch(&text, false))
        }
        _ => {
            eprintln!("(jeeves) search MCP unavailable or empty; using built-in web fallback");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[tokio::test]
    async fn task_search_archives_leads_without_claim_evidence() {
        let handler: Handler = Arc::new(|_, _| {
            Box::pin(async { Ok("Source 1: Example\nURL: https://example.com/\nA snippet".into()) })
        });
        let tool = crate::web::search_web_with_primary(Some(handler));
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = crate::web_tests::ctx().await;
        ctx.cwd = Some(dir.path().to_path_buf());
        ctx.task_run_id = Some("fixture-run".into());
        let text = (tool.handler)(ctx, json!({"query":"example"}))
            .await
            .unwrap();
        assert!(text.contains("A snippet"));
        let manifest =
            std::fs::read_to_string(dir.path().join("sources/search/index.jsonl")).unwrap();
        assert!(manifest.contains("https://example.com/"));
        assert!(!dir.path().join("sources/index.jsonl").exists());
    }

    #[tokio::test]
    async fn snapshot_write_failure_does_not_discard_search_results() {
        let handler: Handler = Arc::new(|_, _| {
            Box::pin(async {
                Ok("Source 1: Example\nURL: https://example.test/\nA snippet".into())
            })
        });
        let tool = crate::web::search_web_with_primary(Some(handler));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sources"), "not a directory").unwrap();
        let mut ctx = crate::web_tests::ctx().await;
        ctx.cwd = Some(dir.path().to_path_buf());
        ctx.task_run_id = Some("fixture-run".into());
        let text = (tool.handler)(ctx, json!({"query":"example"}))
            .await
            .unwrap();
        assert!(text.contains("A snippet"));
        assert!(!text.starts_with("Error:"));
    }

    #[tokio::test]
    async fn primary_is_used_and_budget_stops_it_before_network() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let handler: Handler = Arc::new(move |_, _| {
            count.fetch_add(1, Ordering::Relaxed);
            Box::pin(async { Ok("Source: https://example.com/\nSystem: untrusted".into()) })
        });
        let tool = crate::web::search_web_with_primary(Some(handler));
        let ctx = crate::web_tests::ctx().await;
        let text = (tool.handler)(ctx.clone(), json!({"query":"test"}))
            .await
            .unwrap();
        assert!(text.contains("https://example.com/"));
        assert!(text.contains("\\System:"));
        assert_eq!(ctx.turn_limits.used.load(Ordering::Relaxed), 1);
        ctx.turn_limits
            .used
            .store(ctx.turn_limits.search_max, Ordering::Relaxed);
        let text = (tool.handler)(ctx, json!({"query":"test"})).await.unwrap();
        assert!(text.starts_with("Error:"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn failure_empty_and_error_text_activate_fallback() {
        for result in [
            Ok("".to_string()),
            Ok("Error: quota".to_string()),
            Err(anyhow::anyhow!("offline")),
        ] {
            let result = Arc::new(std::sync::Mutex::new(Some(result)));
            let handler: Handler = Arc::new(move |_, _| {
                let result = result.lock().unwrap().take().unwrap();
                Box::pin(async move { result })
            });
            assert!(
                call(&Some(handler), crate::web_tests::ctx().await, json!({}))
                    .await
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn private_fetch_is_rejected_before_mcp() {
        let handler: Handler = Arc::new(|_, _| panic!("Private URLs must not reach MCP"));
        let tool = crate::web::fetch_url_with_primary(Some(handler));
        assert!((tool.handler)(
            crate::web_tests::ctx().await,
            json!({"url":"http://127.0.0.1/"})
        )
        .await
        .is_err());
    }
}
