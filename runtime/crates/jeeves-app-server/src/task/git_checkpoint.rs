//! Human-only checkpoints in the linked task worktree. Never touches the user's checkout,
//! stages unspecified files, pushes, runs hooks, or rewrites history.
use crate::Server;
use jeeves_protocol::{
    codes, ErrorObject, TaskGitCheckpointParams, TaskGitCheckpointPolicyParams,
    TaskGitCheckpointPolicyResponse, TaskGitCheckpointResponse,
};
use std::{
    path::{Component, Path},
    process::Command,
};

fn rejected(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let empty_hooks = tempfile::tempdir().map_err(|e| e.to_string())?;
    let out = Command::new("git")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .arg("--literal-pathspecs")
        .arg("-C")
        .arg(root)
        .arg("-c")
        .arg(format!("core.hooksPath={}", empty_hooks.path().display()))
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "user.name=Jeeves",
            "-c",
            "user.email=jeeves@localhost",
        ])
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches(['\r', '\n'])
            .into())
    } else {
        Err("git refused the checkpoint operation; inspect the task worktree".into())
    }
}

fn check_path(root: &Path, value: &str, allow_future: bool) -> Result<bool, String> {
    let path = Path::new(value);
    if value.is_empty() || value.len() > 1024 || value.contains(['\r', '\n', '\0']) {
        return Err("invalid checkpoint path".into());
    }
    let mut current = root.to_path_buf();
    let mut metadata = None;
    for component in path.components() {
        let Component::Normal(part) = component else {
            return Err("checkpoint paths must be relative files".into());
        };
        if part.to_string_lossy().eq_ignore_ascii_case(".git") {
            return Err("Git metadata cannot be checkpointed".into());
        }
        current.push(part);
        metadata = match std::fs::symlink_metadata(&current) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("symbolic links cannot be checkpointed".into())
            }
            Ok(m) => Some(m),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("checkpoint path could not be checked".into()),
        };
    }
    match metadata {
        None => {
            let tracked = git(root, &["ls-files", "-z", "--", value])?;
            if tracked == format!("{value}\0") {
                return Ok(true);
            }
            if allow_future && tracked.is_empty() {
                return Ok(false);
            }
            return Err("a missing checkpoint path must name exactly one tracked file".into());
        }
        Some(m) if m.is_dir() => return Err("select explicit files, not directories".into()),
        Some(m) if !m.is_file() => return Err("checkpoint paths must be regular files".into()),
        Some(_) => {}
    }
    let filter = git(root, &["check-attr", "filter", "--", value])?;
    if !filter.ends_with(": filter: unspecified") && !filter.ends_with(": filter: unset") {
        return Err("files with external Git clean filters need a manual Git workflow".into());
    }
    Ok(true)
}

pub(super) fn validate_root(root: &Path, branch: &str) -> Result<(), String> {
    if !root.join(".git").is_file()
        || root.canonicalize().map_err(|e| e.to_string())? != root
        || git(root, &["rev-parse", "--show-toplevel"])? != root.to_string_lossy()
        || git(root, &["branch", "--show-current"])? != branch
    {
        return Err("not the expected linked task worktree and branch".into());
    }
    let admin = std::path::PathBuf::from(git(root, &["rev-parse", "--absolute-git-dir"])?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let managed = root
        .parent()
        .ok_or("task worktree has no parent directory")?;
    if admin.starts_with(managed) {
        return Err("Git administration must be outside the managed worktrees".into());
    }
    let backlink = std::fs::read_to_string(admin.join("gitdir")).map_err(|e| e.to_string())?;
    let backlink = admin.join(backlink.trim_end_matches(['\r', '\n']));
    if backlink != root.join(".git")
        && backlink.canonicalize().ok().as_deref() != Some(&root.join(".git"))
    {
        return Err("Git administration belongs to a different worktree".into());
    }
    Ok(())
}

fn selected_paths<'a>(
    root: &Path,
    paths: &'a [String],
    allow_future: bool,
) -> Result<Vec<&'a str>, String> {
    if paths.is_empty() || paths.len() > 32 {
        return Err("select between 1 and 32 explicit file paths".into());
    }
    let mut selected = Vec::new();
    for path in paths {
        if check_path(root, path, allow_future)? {
            selected.push(path.as_str());
        }
    }
    Ok(selected)
}

fn commit(root: &Path, branch: &str, paths: &[String], automatic: bool) -> Result<String, String> {
    validate_root(root, branch)?;
    let selected = selected_paths(root, paths, automatic)?;
    if !git(root, &["diff", "--cached", "--name-only"])?.is_empty() {
        return Err("existing staged changes must be reviewed before making a checkpoint".into());
    }
    if selected.is_empty() {
        return git(root, &["rev-parse", "HEAD"]);
    }
    let mut add = vec!["add", "-A", "--"];
    add.extend(selected.iter().copied());
    git(root, &add)?;
    if git(root, &["diff", "--cached", "--name-only"])?.is_empty() {
        return git(root, &["rev-parse", "HEAD"]);
    }
    let mut args = vec!["commit", "--only", "-m", "Jeeves task checkpoint", "--"];
    args.extend(selected);
    git(root, &args)?;
    git(root, &["rev-parse", "HEAD"])
}

pub(crate) async fn create(
    server: &Server,
    p: TaskGitCheckpointParams,
) -> Result<TaskGitCheckpointResponse, ErrorObject> {
    if !p.confirmed {
        return Err(rejected("explicit human confirmation is required"));
    }
    let lock = server.session_write_lock(&format!("task-control:{}", p.task_id));
    let _guard = lock.lock().await;
    let task = server
        .tasks
        .get(&p.task_id)
        .await
        .map_err(rejected)?
        .ok_or_else(|| rejected("task does not exist"))?;
    if task.status == jeeves_state::TaskStatus::Running || task.current_turn_id.is_some() {
        return Err(rejected(
            "pause or finish the task before creating a Git checkpoint",
        ));
    }
    let (root, branch) =
        super::worktree::of(&server.settings().home, &task.id, task.cwd.as_deref())
            .ok_or_else(|| rejected("only a Jeeves linked task worktree can be checkpointed"))?;
    let branch_for_commit = branch.clone();
    let hash = tokio::task::spawn_blocking(move || {
        commit(Path::new(&root), &branch_for_commit, &p.paths, false)
    })
    .await
    .map_err(rejected)?
    .map_err(rejected)?;
    server.events.append(&task.id,None,"git_checkpoint",Some(&hash)).await.map_err(|_|rejected(format!("commit {hash} exists but its task event could not be recorded; inspect the worktree")))?;
    eprintln!("(jeeves) task {}: local Git checkpoint {hash}", task.id);
    Ok(TaskGitCheckpointResponse {
        task_id: task.id,
        commit: hash,
        branch,
    })
}

pub(crate) async fn policy(
    server: &Server,
    task_id: &str,
) -> Result<TaskGitCheckpointPolicyResponse, ErrorObject> {
    let paths = jeeves_state::git_checkpoints::policy(&server.pool, task_id)
        .await
        .map_err(rejected)?;
    Ok(TaskGitCheckpointPolicyResponse {
        task_id: task_id.into(),
        enabled: paths.is_some(),
        paths: paths.unwrap_or_default(),
    })
}

pub(crate) async fn set_policy(
    server: &Server,
    p: TaskGitCheckpointPolicyParams,
) -> Result<TaskGitCheckpointPolicyResponse, ErrorObject> {
    if !p.confirmed {
        return Err(rejected("explicit human confirmation required"));
    }
    let lock = server.session_write_lock(&format!("task-control:{}", p.task_id));
    let _guard = lock.lock().await;
    let task = server
        .tasks
        .get(&p.task_id)
        .await
        .map_err(rejected)?
        .ok_or_else(|| rejected("task does not exist"))?;
    if task.status == jeeves_state::TaskStatus::Running || task.current_turn_id.is_some() {
        return Err(rejected(
            "stop the task before changing the checkpoint policy",
        ));
    }
    let (root, branch) =
        super::worktree::of(&server.settings().home, &task.id, task.cwd.as_deref()).ok_or_else(
            || rejected("only an isolated task worktree supports automatic checkpoints"),
        )?;
    if p.enabled {
        let paths = p.paths.clone();
        tokio::task::spawn_blocking(move || {
            let root = Path::new(&root);
            validate_root(root, &branch)?;
            selected_paths(root, &paths, true)?;
            Ok::<(), String>(())
        })
        .await
        .map_err(rejected)?
        .map_err(rejected)?;
    }
    jeeves_state::git_checkpoints::set_policy(
        &server.pool,
        &task.id,
        p.enabled.then_some(p.paths.as_slice()),
    )
    .await
    .map_err(rejected)?;
    eprintln!(
        "(jeeves) task {}: automatic Git checkpoints {}",
        task.id,
        if p.enabled {
            "enabled for human-selected paths"
        } else {
            "disabled"
        }
    );
    policy(server, &task.id).await
}

/// Called after the state checkpoint commits, before the next model call. A failed Git
/// checkpoint stops execution rather than claiming a snapshot we cannot identify.
pub(crate) async fn record_boundary(
    server: &Server,
    turn_id: &str,
    run_id: &str,
    digest: &str,
) -> Result<(), String> {
    let Some(run) = server.runs.get(run_id).await.map_err(|e| e.to_string())? else {
        return Err("task execution missing at checkpoint".into());
    };
    if jeeves_state::git_checkpoints::policy(&server.pool, &run.task_id)
        .await
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Ok(());
    }
    let lock = server.session_write_lock(&format!("task-control:{}", run.task_id));
    let _guard = lock.lock().await;
    let Some(paths) = jeeves_state::git_checkpoints::policy(&server.pool, &run.task_id)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(());
    };
    let task = server
        .tasks
        .get(&run.task_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("checkpoint task missing")?;
    if task.status != jeeves_state::TaskStatus::Running
        || task.current_turn_id.as_deref() != Some(turn_id)
    {
        return Ok(());
    }
    if jeeves_state::git_checkpoints::boundary(&server.pool, &task.id, turn_id, digest)
        .await
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Ok(());
    }
    let (root, branch) =
        super::worktree::of(&server.settings().home, &task.id, task.cwd.as_deref())
            .ok_or("checkpoint worktree identity changed")?;
    let sha = tokio::task::spawn_blocking(move || commit(Path::new(&root), &branch, &paths, true))
        .await
        .map_err(|e| e.to_string())??;
    if jeeves_state::git_checkpoints::record(&server.pool, &task.id, turn_id, digest, &sha)
        .await
        .map_err(|e| e.to_string())?
    {
        server
            .events
            .append(&task.id, Some(run_id), "git_checkpoint", Some(&sha))
            .await
            .map_err(|e| e.to_string())?;
        eprintln!("(jeeves) task {}: automatic Git checkpoint {sha}", task.id);
    }
    Ok(())
}
