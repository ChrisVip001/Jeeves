//! Host-injected search service; local implementations remain the fallback.

use crate::{Handler, ToolCtx};
use serde_json::Value;

pub(super) async fn call(primary: &Option<Handler>, ctx: ToolCtx, args: Value) -> Option<String> {
    let handler = primary.as_ref()?;
    match handler(ctx, args).await {
        Ok(text) if !text.trim().is_empty() => Some(super::finish_fetch(&text, false)),
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
        Arc,
        atomic::{AtomicUsize, Ordering},
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
        let error = (tool.handler)(ctx, json!({"query":"test"}))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("limit"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn exact_search_retries_share_one_success_within_a_turn() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let handler: Handler = Arc::new(move |_, _| {
            let request = count.fetch_add(1, Ordering::Relaxed);
            Box::pin(async move {
                tokio::task::yield_now().await;
                Ok(format!("Search fixture retrieval #{request}"))
            })
        });
        let tool = crate::web::search_web_with_primary(Some(handler));
        let ctx = crate::web_tests::ctx().await;
        let args = json!({"query":"示例市科技馆 现行闭馆时间"});
        let (first, repeated) = tokio::join!(
            (tool.handler)(ctx.clone(), args.clone()),
            (tool.handler)(ctx.clone(), args.clone())
        );
        assert_eq!(first.unwrap(), repeated.unwrap());
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        for different in [
            json!({"query":"示例市科技馆 现行闭馆时间","max_results":3}),
            json!({"query":"示例市科技馆 票价"}),
        ] {
            (tool.handler)(ctx.clone(), different).await.unwrap();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        (tool.handler)(crate::web_tests::ctx().await, args.clone())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 4);

        assert_eq!(ctx.turn_limits.used.load(Ordering::Relaxed), 4);
    }

    #[tokio::test]
    async fn failed_search_can_be_retried() {
        let limits = crate::web::TurnLimits::default();
        assert!(
            limits
                .search_once("still relevant", 5, async {
                    anyhow::bail!("temporary search failure")
                })
                .await
                .is_err()
        );
        let output = limits
            .search_once("still relevant", 5, async { Ok("now available".into()) })
            .await
            .unwrap();
        assert_eq!(output, "now available");
    }

    #[tokio::test]
    async fn failure_empty_and_error_text_activate_fallback() {
        for result in [Ok("".to_string()), Err(anyhow::anyhow!("offline"))] {
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
        assert!(
            (tool.handler)(
                crate::web_tests::ctx().await,
                json!({"url":"http://127.0.0.1/"})
            )
            .await
            .is_err()
        );
    }
}
