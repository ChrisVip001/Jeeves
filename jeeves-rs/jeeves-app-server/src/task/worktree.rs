//! Coding tasks run in their own git worktree and branch, never in the human's checkout.
use super::git_checkpoint::git;
use jeeves_protocol::{codes, ErrorObject};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::Command;

fn refuse(message: String) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: format!("Task repository was not usable: {message}"),
        data: None,
    }
}

pub(super) fn branch_of(task_id: &str) -> String {
    format!("jeeves/{task_id}")
}

/// Offer an isolated checkout for a clean repository selected by the owner.
pub(super) async fn clean_repository(root: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        if !root.join(".git").exists() {
            return None;
        }
        let root = root.canonicalize().ok()?;
        let repo = git(&root, &["rev-parse", "--show-toplevel"]).ok()?;
        let status = git(
            &root,
            &["status", "--porcelain", "--untracked-files=normal"],
        )
        .ok()?;
        (repo == root.to_string_lossy() && status.is_empty()).then_some(repo)
    })
    .await
    .ok()
    .flatten()
}

/// Create (or reuse) `<home>/worktrees/<task-id>` on branch `jeeves/<task-id>` for `repo`.
/// The result is the task's only writable root. Never deletes anything.
pub(super) async fn ensure(
    home: PathBuf,
    task_id: String,
    repo: String,
) -> Result<PathBuf, ErrorObject> {
    tokio::task::spawn_blocking(move || {
        let repo = PathBuf::from(&repo);
        if !repo.is_absolute() {
            return Err(refuse(format!(
                "{} is not an absolute path",
                repo.display()
            )));
        }
        if !repo.is_dir() {
            return Err(refuse(format!("{} is not a directory", repo.display())));
        }
        git(&repo, &["rev-parse", "--show-toplevel"])
            .map_err(|e| refuse(format!("{} is not a git repository ({e})", repo.display())))?;
        let home = home.canonicalize().map_err(|e| refuse(e.to_string()))?;
        let root = home.join("worktrees");
        std::fs::create_dir_all(&root).map_err(|e| refuse(e.to_string()))?;
        if root.canonicalize().map_err(|e| refuse(e.to_string()))? != root {
            return Err(refuse(
                "managed worktrees must be inside the canonical home".into(),
            ));
        }
        let path = root.join(&task_id);
        if path.join(".git").exists() {
            if path.canonicalize().map_err(|e| refuse(e.to_string()))? != path
                || git(&path, &["branch", "--show-current"]).map_err(refuse)? != branch_of(&task_id)
            {
                return Err(refuse(
                    "existing task worktree has a different owner".into(),
                ));
            }
            return Ok(path);
        }
        let target = path.display().to_string();
        git(
            &repo,
            &["worktree", "add", &target, "-b", &branch_of(&task_id)],
        )
        .map_err(|e| refuse(format!("git worktree add failed: {e}")))?;
        path.canonicalize().map_err(|e| refuse(e.to_string()))
    })
    .await
    .map_err(|e| refuse(e.to_string()))?
}

/// Recover the original boundary from the persisted task, independent of a turn's requested cwd.
pub(crate) fn owned(home: &Path, task: Option<&jeeves_state::TaskRow>) -> Option<PathBuf> {
    let task = task?;
    managed_path(home, &task.id, task.cwd.as_deref()).map(Path::to_path_buf)
}

fn managed_path<'a>(home: &Path, id: &str, cwd: Option<&'a str>) -> Option<&'a Path> {
    let path = Path::new(cwd?);
    if path.file_name()? != id {
        return None;
    }
    let worktrees = home
        .canonicalize()
        .unwrap_or_else(|_| home.to_owned())
        .join("worktrees");
    let parent = path.parent()?;
    (parent == worktrees || worktrees.canonicalize().ok().as_deref() == Some(parent))
        .then_some(path)
}

/// The worktree and branch behind a task's `cwd`, when it is one of ours.
pub(super) fn of(home: &Path, task_id: &str, cwd: Option<&str>) -> Option<(String, String)> {
    let path = managed_path(home, task_id, cwd)?;
    Some((path.display().to_string(), branch_of(task_id)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "i",
            ],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        dir
    }

    #[tokio::test]
    async fn worktree_is_created_once_and_reused() {
        let (home, repo) = (tempfile::tempdir().unwrap(), repo());
        let r = repo.path().display().to_string();
        let a = ensure(home.path().into(), "t1".into(), r.clone())
            .await
            .unwrap();
        assert!(a.join(".git").exists());
        let b = ensure(home.path().into(), "t1".into(), r).await.unwrap();
        assert_eq!(a, b);
        let branch = git(&a, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(branch, "jeeves/t1");
        let (path, br) = of(home.path(), "t1", Some(&a.display().to_string())).unwrap();
        assert_eq!(
            (path, br),
            (a.display().to_string(), "jeeves/t1".to_string())
        );
        assert!(of(home.path(), "t1", Some("/elsewhere")).is_none());
    }

    #[tokio::test]
    async fn non_repository_and_relative_paths_are_refused_as_text() {
        let home = tempfile::tempdir().unwrap();
        let plain = tempfile::tempdir().unwrap();
        let e = ensure(
            home.path().into(),
            "t".into(),
            plain.path().display().to_string(),
        )
        .await
        .unwrap_err();
        assert!(e.message.contains("not a git repository"), "{}", e.message);
        let e = ensure(home.path().into(), "t".into(), "rel/path".into())
            .await
            .unwrap_err();
        assert!(e.message.contains("absolute"));
        assert!(!home.path().join("worktrees/t").exists());
    }
}
