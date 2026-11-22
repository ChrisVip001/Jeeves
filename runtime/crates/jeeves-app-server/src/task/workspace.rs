//! Managed task roots. A conversation never becomes an accidental shared write boundary.
use crate::Server;
use jeeves_protocol::{codes, ErrorObject};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) fn task_id() -> String {
    format!(
        "task{}-{}",
        chrono::Utc::now().timestamp_micros(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

pub(super) async fn allocate(
    server: &Server,
    id: &str,
    title: &str,
) -> Result<PathBuf, ErrorObject> {
    let home = server.settings().home;
    let id = id.to_string();
    let title: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .take(32)
        .collect();
    tokio::task::spawn_blocking(move || {
        let home = home.canonicalize()?;
        let root = home.join("workspaces");
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        if !root.starts_with(&home) {
            return Err(std::io::Error::other(
                "Managed workspaces must stay inside Jeeves home",
            ));
        }
        let name = format!(
            "{}-{}-{}",
            chrono::Local::now().format("%Y-%m-%d"),
            if title.is_empty() { "task" } else { &title },
            id
        );
        let path = root.join(name);
        std::fs::create_dir(&path)?;
        path.canonicalize()
    })
    .await
    .map_err(|e| failure(e.to_string()))?
    .map_err(|e| failure(e.to_string()))
}
fn failure(message: String) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: format!("Task workspace could not be created: {message}"),
        data: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn roots_are_unique_contained_and_created_without_a_session_directory() {
        let home = tempfile::tempdir().unwrap();
        let server = Server::boot(
            jeeves_state::open(&home.path().join("state.db"))
                .await
                .unwrap(),
            jeeves_config::Settings {
                home: home.path().into(),
                ..Default::default()
            },
        )
        .await;
        let a = allocate(&server, &task_id(), "../../报告\\计划")
            .await
            .unwrap();
        let b = allocate(&server, &task_id(), "../../报告\\计划")
            .await
            .unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(home.path().canonicalize().unwrap().join("workspaces")));
        assert!(a.is_dir());
    }
}
