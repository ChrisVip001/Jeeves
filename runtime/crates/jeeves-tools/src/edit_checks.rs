//! Diagnostics right after an edit: a task can declare checks (`after: edit`) that run in its
//! worktree after every `edit` / `apply_patch`, and their output rides on the same tool result.
//! The tool layer knows only this trait; the host implements it.
use crate::ToolCtx;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

pub type CheckFut = Pin<Box<dyn Future<Output = Option<String>> + Send>>;

pub trait EditChecks: Send + Sync {
    /// The text to append to an edit's result (check name, outcome, up to 4 KB of output), or
    /// `None` when this run has no edit checks.
    fn after_edit(
        &self,
        run_id: Option<String>,
        cwd: Option<PathBuf>,
        lease: Option<crate::resources::Lease>,
    ) -> CheckFut;
}

/// Append the checks' output to `result`. Never fails the edit: whatever the checks do, the
/// edit already landed and its own result stays first.
pub async fn append(ctx: &ToolCtx, result: String) -> String {
    append_with(
        ctx.edit_checks.as_ref(),
        ctx.task_run_id.clone(),
        ctx.cwd.clone(),
        ctx.resource_lease.clone(),
        result,
    )
    .await
}

async fn append_with(
    checks: Option<&std::sync::Arc<dyn EditChecks>>,
    run_id: Option<String>,
    cwd: Option<PathBuf>,
    lease: Option<crate::resources::Lease>,
    result: String,
) -> String {
    let Some(checks) = checks else {
        return result;
    };
    match checks.after_edit(run_id, cwd, lease).await {
        Some(extra) => format!("{result}\n{extra}"),
        None => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Fixed(Option<&'static str>);
    impl EditChecks for Fixed {
        fn after_edit(
            &self,
            _: Option<String>,
            _: Option<PathBuf>,
            _: Option<crate::resources::Lease>,
        ) -> CheckFut {
            let text = self.0.map(str::to_string);
            Box::pin(async move { text })
        }
    }

    #[tokio::test]
    async fn output_follows_the_edit_result_and_absence_changes_nothing() {
        let none = None;
        assert_eq!(
            append_with(none, None, None, None, "Edited".into()).await,
            "Edited"
        );
        let some: Arc<dyn EditChecks> = Arc::new(Fixed(Some("[check cargo] exit 1\nerror[E0308]")));
        assert_eq!(
            append_with(Some(&some), None, None, None, "Edited".into()).await,
            "Edited\n[check cargo] exit 1\nerror[E0308]"
        );
        let silent: Arc<dyn EditChecks> = Arc::new(Fixed(None));
        assert_eq!(
            append_with(Some(&silent), None, None, None, "Edited".into()).await,
            "Edited"
        );
    }
}
