//! Read-only, bounded delivery snapshots. The browser never reads a live workspace path.
use crate::Server;
use jeeves_protocol::{
    ErrorObject, TaskReviewCaptureParams, TaskReviewFile, TaskReviewView, codes,
};
use jeeves_state::{TaskRow, TaskStatus};
use std::{
    io::Read,
    path::{Component, Path},
    process::{Command, Stdio},
};

const FILE_LIMIT: usize = 64 * 1024;
const PATCH_LIMIT: usize = 192 * 1024;
const TOTAL_LIMIT: usize = 2 * 1024 * 1024;
const PATH_LIMIT: usize = 100;

fn invalid(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

fn text(bytes: Vec<u8>) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn read_output(
    root: &Path,
    args: &[&str],
    limit: usize,
    diff: bool,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut child = Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.fsmonitor=false", "-c", "core.quotePath=false"])
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    let result = child
        .stdout
        .take()
        .expect("piped git output")
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes);
    if result.is_err() || bytes.len() > limit {
        let _ = child.kill();
        child.wait()?;
        result?;
        return Ok(None);
    }
    let status = child.wait()?;
    anyhow::ensure!(
        status.success() || (diff && status.code() == Some(1)),
        "Git could not read the delivery"
    );
    Ok(Some(bytes))
}

fn names(root: &Path, args: &[&str]) -> anyhow::Result<Vec<String>> {
    let raw = read_output(root, args, TOTAL_LIMIT, false)?
        .ok_or_else(|| anyhow::anyhow!("too many changed paths to list"))?;
    Ok(String::from_utf8(raw)?
        .split_terminator('\0')
        .map(str::to_owned)
        .collect())
}

fn revision(value: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Use a full Git commit ID"
    );
    Ok(format!("{value}^{{commit}}"))
}

fn blob(root: &Path, commit: &str, path: &str) -> anyhow::Result<Option<String>> {
    let object = format!("{commit}:{path}");
    if super::git_checkpoint::git(root, &["cat-file", "-t", &object]).map_err(anyhow::Error::msg)?
        != "blob"
    {
        return Ok(None);
    }
    Ok(read_output(root, &["cat-file", "blob", &object], FILE_LIMIT, false)?.and_then(text))
}

fn working_file(root: &Path, path: &str) -> anyhow::Result<Option<String>> {
    let mut resolved = root.to_owned();
    for part in Path::new(path).components() {
        let Component::Normal(name) = part else {
            return Ok(None);
        };
        resolved.push(name);
        if std::fs::symlink_metadata(&resolved)?
            .file_type()
            .is_symlink()
        {
            return Ok(None);
        }
    }
    if !resolved.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    std::fs::File::open(resolved)?
        .take((FILE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok((bytes.len() <= FILE_LIMIT).then(|| text(bytes)).flatten())
}

fn patch(temp: &Path, before: Option<&str>, after: Option<&str>) -> anyhow::Result<Option<String>> {
    std::fs::write(temp.join("before"), before.unwrap_or_default())?;
    std::fs::write(temp.join("after"), after.unwrap_or_default())?;
    let out = read_output(
        temp,
        &[
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            "before",
            "after",
        ],
        PATCH_LIMIT,
        true,
    )?
    .and_then(text);
    Ok(out.map(|value| {
        value
            .find("\n@@ ")
            .map_or(String::new(), |index| value[index + 1..].to_owned())
    }))
}

fn snapshot(root: &Path, base: String, result: Option<String>) -> anyhow::Result<TaskReviewView> {
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--name-status",
        "-z",
        &base,
    ];
    args.extend(result.as_deref());
    args.push("--");
    let paths = names(root, &args)?;
    let mut entries = paths.iter();
    let mut changes = Vec::new();
    while let (Some(status), Some(path)) = (entries.next(), entries.next()) {
        changes.push((status.as_str(), path.as_str()));
    }
    let untracked;
    if result.is_none() {
        untracked = names(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
        changes.extend(untracked.iter().map(|name| ("A", name.as_str())));
    }
    changes.sort_unstable_by(|left, right| left.1.cmp(right.1));
    changes.dedup_by_key(|entry| entry.1);
    let count = changes.len();
    let mut files = Vec::new();
    let mut used = 0;
    let temp = tempfile::tempdir()?;
    for (kind, path) in changes.into_iter().take(PATH_LIMIT) {
        let before = if kind == "A" {
            None
        } else {
            blob(root, &base, path)?
        };
        let after = if kind == "D" {
            None
        } else if let Some(commit) = result.as_deref() {
            blob(root, commit, path)?
        } else {
            working_file(root, path)?
        };
        let patch = if (before.is_some() || kind == "A") && (after.is_some() || kind == "D") {
            patch(temp.path(), before.as_deref(), after.as_deref())?
        } else {
            None
        };
        used += path.len()
            + after.as_ref().map_or(0, String::len)
            + patch.as_ref().map_or(0, String::len);
        if used > TOTAL_LIMIT {
            break;
        }
        files.push(TaskReviewFile {
            path: path.into(),
            change: match kind {
                "A" => "added",
                "D" => "deleted",
                _ => "modified",
            }
            .into(),
            content: after,
            patch,
        });
    }
    Ok(TaskReviewView {
        base_commit: base,
        result_commit: result,
        omitted_files: crate::narrow((count - files.len()) as i64),
        files,
    })
}

async fn store(
    server: &Server,
    task: &str,
    run: &str,
    view: &TaskReviewView,
) -> anyhow::Result<()> {
    sqlx::query("INSERT INTO task_reviews(task_id,run_id,review_json) VALUES(?,?,?) ON CONFLICT(task_id,run_id) DO NOTHING")
        .bind(task).bind(run).bind(serde_json::to_string(view)?).execute(&server.pool).await?;
    Ok(())
}

pub(super) async fn load(
    server: &Server,
    task: &str,
    run: &str,
) -> anyhow::Result<Option<TaskReviewView>> {
    let json: Option<String> =
        sqlx::query_scalar("SELECT review_json FROM task_reviews WHERE task_id=? AND run_id=?")
            .bind(task)
            .bind(run)
            .fetch_optional(&server.pool)
            .await?;
    Ok(json.map(|value| serde_json::from_str(&value)).transpose()?)
}

pub(super) async fn remember_base(server: &Server, task: &str, root: &Path) -> anyhow::Result<()> {
    let root = root.to_owned();
    let base = tokio::task::spawn_blocking(move || {
        super::git_checkpoint::git(&root, &["rev-parse", "HEAD"])
    })
    .await?
    .map_err(anyhow::Error::msg)?;
    sqlx::query("INSERT INTO task_review_bases(task_id,base_commit) VALUES(?,?)")
        .bind(task)
        .bind(base)
        .execute(&server.pool)
        .await?;
    Ok(())
}

pub(crate) async fn at_delivery(server: &Server, task: &TaskRow, run: &str) -> anyhow::Result<()> {
    let base: Option<String> =
        sqlx::query_scalar("SELECT base_commit FROM task_review_bases WHERE task_id=?")
            .bind(&task.id)
            .fetch_optional(&server.pool)
            .await?;
    let Some(base) = base else { return Ok(()) };
    let Some(root) = super::worktree::owned(&server.settings().home, Some(task)) else {
        return Ok(());
    };
    let branch = super::worktree::branch_of(&task.id);
    let view = tokio::task::spawn_blocking(move || {
        super::git_checkpoint::validate_root(&root, &branch).map_err(anyhow::Error::msg)?;
        snapshot(&root, base, None)
    })
    .await??;
    store(server, &task.id, run, &view).await
}

pub(crate) async fn capture(
    server: &Server,
    params: TaskReviewCaptureParams,
) -> Result<TaskReviewView, ErrorObject> {
    let lock = server.session_write_lock(&format!("task-control:{}", params.task_id));
    let _guard = lock.lock().await;
    let task = server
        .tasks
        .get(&params.task_id)
        .await
        .map_err(invalid)?
        .ok_or_else(|| invalid("task not found"))?;
    if task.status != TaskStatus::Done || task.current_turn_id.is_some() {
        return Err(invalid("Attach commits to a finished delivery"));
    }
    let run = server
        .completions
        .latest_delivery(&task.id)
        .await
        .map_err(invalid)?
        .and_then(|row| row.run_id)
        .ok_or_else(|| invalid("task has no recorded delivery run"))?;
    if let Some(existing) = load(server, &task.id, &run).await.map_err(invalid)? {
        if existing.base_commit == params.base_commit
            && existing.result_commit.as_deref() == Some(&params.result_commit)
        {
            return Ok(existing);
        }
        return Err(invalid("The delivery already has a file snapshot"));
    }
    let root = task.cwd.ok_or_else(|| invalid("task has no workspace"))?;
    let view = tokio::task::spawn_blocking(move || {
        let root = Path::new(&root).canonicalize()?;
        let git =
            |args: &[&str]| super::git_checkpoint::git(&root, args).map_err(anyhow::Error::msg);
        anyhow::ensure!(
            git(&["rev-parse", "--show-toplevel"])? == root.to_string_lossy(),
            "task workspace must be the repository root"
        );
        for id in [&params.base_commit, &params.result_commit] {
            anyhow::ensure!(
                git(&["rev-parse", "--verify", &revision(id)?])? == *id,
                "Invalid Git commit"
            );
        }
        git(&[
            "merge-base",
            "--is-ancestor",
            &params.base_commit,
            &params.result_commit,
        ])?;
        snapshot(&root, params.base_commit, Some(params.result_commit))
    })
    .await
    .map_err(invalid)?
    .map_err(invalid)?;
    store(server, &task.id, &run, &view)
        .await
        .map_err(invalid)?;
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_added_modified_and_deleted_files_without_reading_the_new_checkout() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path();
        let git = |args: &[&str]| super::super::git_checkpoint::git(dir, args).unwrap();
        git(&["init"]);
        std::fs::write(dir.join("menu.txt"), "original\nshared\n").unwrap();
        std::fs::write(dir.join("retired.txt"), "old\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(dir.join("menu.txt"), "updated\nshared\n").unwrap();
        std::fs::remove_file(dir.join("retired.txt")).unwrap();
        std::fs::write(dir.join("profile.txt"), "added\n").unwrap();
        let working = snapshot(dir, base.clone(), None).unwrap();
        assert_eq!(working.files.len(), 3);
        git(&["add", "-A"]);
        git(&["commit", "-m", "deliver"]);
        let result = git(&["rev-parse", "HEAD"]);
        std::fs::write(dir.join("menu.txt"), "later edits\n").unwrap();

        let view = snapshot(dir, base, Some(result)).unwrap();
        let changed = &view.files[0];
        assert_eq!(changed.path, "menu.txt");
        assert_eq!(changed.content.as_deref(), Some("updated\nshared\n"));
        assert_eq!(
            changed.patch.as_deref(),
            Some("@@ -1,2 +1,2 @@\n-original\n+updated\n shared\n")
        );
        assert_eq!(view.files[1].change, "added");
        assert_eq!(
            view.files[1].patch.as_deref(),
            Some("@@ -0,0 +1 @@\n+added\n")
        );
        assert_eq!(view.files[2].change, "deleted");
        assert_eq!(view.files[2].content, None);
        assert_eq!(
            view.files[2].patch.as_deref(),
            Some("@@ -1 +0,0 @@\n-old\n")
        );
    }
}
