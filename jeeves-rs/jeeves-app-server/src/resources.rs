//! Unscoped mutations exclude all writers. Disjoint task worktrees may edit concurrently.
use jeeves_tools::resources::{Acquire, Lease, ResourceCoordinator, ResourceLease};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

#[derive(Default)]
pub(crate) struct Mutations {
    gate: Arc<RwLock<()>>,
    worktrees: Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>,
}

struct Permit<G> {
    _guard: G,
}
impl<G: Send + Sync> ResourceLease for Permit<G> {}

impl ResourceCoordinator for Mutations {
    fn acquire(&self, operation: &str, worktree: Option<&Path>) -> Acquire {
        let gate = self.gate.clone();
        let local = worktree
            .filter(|_| {
                matches!(
                    operation,
                    "write_file" | "edit" | "apply_patch" | "run_command"
                )
            })
            .map(|root| {
                let mut locks = self.worktrees.lock().expect("worktree admission lock");
                locks.retain(|_, lock| lock.strong_count() > 0);
                if let Some(lock) = locks.get(root).and_then(Weak::upgrade) {
                    return lock;
                }
                let lock = Arc::new(AsyncMutex::new(()));
                locks.insert(root.into(), Arc::downgrade(&lock));
                lock
            });
        let operation = operation.to_owned();
        Box::pin(async move {
            let acquire = async move {
                if let Some(local) = local {
                    let shared = gate.read_owned().await;
                    let writer = local.lock_owned().await;
                    Arc::new(Permit {
                        _guard: (shared, writer),
                    }) as Lease
                } else {
                    Arc::new(Permit {
                        _guard: gate.write_owned().await,
                    }) as Lease
                }
            };
            tokio::pin!(acquire);
            tokio::select! {
                biased;
                permit = &mut acquire => return Ok(permit),
                _ = tokio::task::yield_now() => {}
            }
            eprintln!(
                "(jeeves) {operation} waiting for another mutation or background job to settle"
            );
            tokio::time::timeout(std::time::Duration::from_secs(2), acquire).await
                .map_err(|_| "Another mutation or background job still owns the execution resources; inspect its progress before retrying".into())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cloned_permits_retain_exclusion_and_cancellation_releases_it() {
        let coordinator = Mutations::default();
        let lease = coordinator.acquire("first", None).await.unwrap();
        let held = lease.clone();
        drop(lease);
        assert!(coordinator.gate.clone().try_write_owned().is_err());
        drop(held);
        assert!(coordinator.gate.clone().try_write_owned().is_ok());
        let lease = coordinator.acquire("holder", None).await.unwrap();
        let task = tokio::spawn(coordinator.acquire("cancelled", None));
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        drop(lease);
        assert!(coordinator.acquire("next", None).await.is_ok());
    }

    #[tokio::test]
    async fn background_jobs_block_other_session_writes_until_settled() {
        if !jeeves_tools::exec::sandbox_available() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let pool = jeeves_state::open(&home.path().join("state.db"))
            .await
            .unwrap();
        let server = crate::Server::boot(
            pool,
            jeeves_config::Settings {
                home: home.path().into(),
                provider: "test-unconfigured".into(),
                exec_enabled: true,
                exec_allow: vec!["sleep*".into()],
                write_enabled: true,
                approval: "never".into(),
                ..Default::default()
            },
        )
        .await;
        let first = server.tool_ctx("task", Some(workspace.path().into()), None);
        let result = server
            .tools
            .execute(
                first,
                "run_command",
                serde_json::json!({"command":"sleep 10","background":true}),
            )
            .await;
        assert!(result.starts_with("Running in the background:"), "{result}");
        let other = server.tool_ctx("chat", Some(workspace.path().into()), None);
        let args = serde_json::json!({"path":"result.txt","content":"second session"});
        let result = server
            .tools
            .execute(other.clone(), "write_file", args.clone())
            .await;
        assert!(
            result.contains("still owns the execution resources"),
            "{result}"
        );
        assert!(!workspace.path().join("result.txt").exists());
        let jobs = server.jobs.as_ref().unwrap();
        let id = jobs.list("task")[0].id.clone();
        jobs.kill("task", &id).await.unwrap();
        let done = jobs.output("task", &id, 0, true).await.unwrap();
        assert!(!done.running);
        let result = server.tools.execute(other, "write_file", args).await;
        assert!(!result.starts_with("Error:"), "{result}");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("result.txt")).unwrap(),
            "second session"
        );
    }
}
