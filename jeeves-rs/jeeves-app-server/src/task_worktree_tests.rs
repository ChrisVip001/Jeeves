use super::input_wake_tests::{fixture, rpc};
use serde_json::json;
use std::process::Command;

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
async fn a_repo_task_runs_in_its_own_worktree_and_the_view_says_so() {
    let (home, server, _model) = fixture().await;
    let repo = repo();
    let made = rpc(
        &server,
        "task/create",
        json!({"title":"fix it","repo":repo.path()}),
    )
    .await
    .unwrap();
    let task = &made["task"];
    let id = task["id"].as_str().unwrap();
    let path = task["worktreePath"].as_str().unwrap();
    assert_eq!(
        task["cwd"], task["worktreePath"],
        "the worktree is the only working directory"
    );
    assert_eq!(task["branch"], format!("jeeves/{id}"));
    assert!(path.starts_with(
        home.path()
            .canonicalize()
            .unwrap()
            .join("worktrees")
            .to_str()
            .unwrap()
    ));
    let got = rpc(&server, "task/get", json!({"taskId": id}))
        .await
        .unwrap();
    assert_eq!(got["task"]["branch"], task["branch"]);
}

#[tokio::test]
async fn checkpoint_commits_only_explicit_files_and_never_the_human_branch() {
    let (_home, server, _model) = fixture().await;
    let repo = repo();
    let head = |path: &std::path::Path| {
        String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };
    let main = head(repo.path());
    let made = rpc(
        &server,
        "task/create",
        json!({"title":"checkpoint fixture","repo":repo.path()}),
    )
    .await
    .unwrap();
    let task = &made["task"];
    let id = task["id"].as_str().unwrap();
    let worktree = std::path::Path::new(task["worktreePath"].as_str().unwrap());
    std::fs::write(worktree.join("approved.txt"), "approved").unwrap();
    std::fs::write(worktree.join("not-approved.txt"), "not approved").unwrap();
    assert!(rpc(
        &server,
        "task/gitCheckpoint",
        json!({"taskId":id,"paths":["approved.txt"],"confirmed":false})
    )
    .await
    .is_err());
    assert!(rpc(
        &server,
        "task/gitCheckpoint",
        json!({"taskId":id,"paths":["../elsewhere"],"confirmed":true})
    )
    .await
    .is_err());
    let result = rpc(
        &server,
        "task/gitCheckpoint",
        json!({"taskId":id,"paths":["approved.txt"],"confirmed":true}),
    )
    .await
    .unwrap();
    assert_eq!(result["commit"], head(worktree));
    assert_ne!(head(worktree), main);
    assert_eq!(head(repo.path()), main);
    let listed = String::from_utf8(
        Command::new("git")
            .arg("-C")
            .arg(worktree)
            .args(["ls-tree", "-r", "--name-only", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert_eq!(listed.trim(), "approved.txt");
    assert!(worktree.join("not-approved.txt").exists());
    assert!(server
        .events
        .list(id, 10)
        .await
        .unwrap()
        .iter()
        .any(|e| e.kind == "git_checkpoint"));

    let file = "中文说明.txt";
    std::fs::write(worktree.join(file), "fixture").unwrap();
    rpc(
        &server,
        "task/gitCheckpoint",
        json!({"taskId":id,"paths":[file],"confirmed":true}),
    )
    .await
    .unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["config", "core.quotePath", "true"])
        .status()
        .unwrap()
        .success());
    std::fs::remove_file(worktree.join(file)).unwrap();
    let saved = rpc(
        &server,
        "task/gitCheckpoint",
        json!({"taskId":id,"paths":[file],"confirmed":true}),
    )
    .await
    .unwrap();
    assert_eq!(saved["commit"], head(worktree));
    assert_eq!(head(repo.path()), main);
    let listed = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["ls-tree", "-r", "--name-only", "HEAD"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8(listed.stdout).unwrap().trim(),
        "approved.txt"
    );
}

#[tokio::test]
async fn automatic_policy_maps_real_execution_boundaries_only_after_human_opt_in() {
    use jeeves_provider::{mock::Mock, Resolved};
    let (_home, server, _model) = fixture().await;
    let repo = repo();
    let made = rpc(
        &server,
        "task/create",
        json!({"title":"auto checkpoint","repo":repo.path()}),
    )
    .await
    .unwrap();
    rpc(&server, "session/permission/set", json!({
        "sessionId":"jeeves-main", "mode":"fullAccess", "expectedRevision":1, "confirmFullAccess":true
    })).await.unwrap();
    let task = &made["task"];
    let id = task["id"].as_str().unwrap();
    let root = std::path::Path::new(task["worktreePath"].as_str().unwrap());
    let git = |path: &std::path::Path, args: &[&str]| {
        String::from_utf8(
            Command::new("git")
                .arg("-C")
                .arg(path)
                .args(args)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    };
    let main = git(repo.path(), &["rev-parse", "HEAD"]);
    std::fs::write(root.join("private.txt"), "not selected").unwrap();
    assert_eq!(
        rpc(&server, "task/gitCheckpointPolicy", json!({"taskId":id}))
            .await
            .unwrap()["enabled"],
        false
    );
    assert!(rpc(
        &server,
        "task/gitCheckpointPolicy/set",
        json!({"taskId":id,"paths":["result.txt"],"enabled":true,"confirmed":false})
    )
    .await
    .is_err());
    assert_eq!(
        rpc(
            &server,
            "task/gitCheckpointPolicy/set",
            json!({"taskId":id,"paths":["result.txt"],"enabled":true,"confirmed":true})
        )
        .await
        .unwrap()["enabled"],
        true
    );
    assert_eq!(
        git(root, &["rev-parse", "HEAD"]),
        main,
        "enabling alone does not commit"
    );
    server.install_provider(Resolved {
        client: std::sync::Arc::new(Mock::new(vec![
            Mock::tool_use(
                "write",
                "write_file",
                json!({"path":"result.txt","content":"fixture result"}),
            ),
            Mock::text("done"),
        ])),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    rpc(
        &server,
        "task/start",
        json!({"taskId":id,"message":"Write result.txt"}),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        loop {
            let status = server.tasks.get(id).await.unwrap().unwrap().status;
            if status == jeeves_state::TaskStatus::Done {
                break;
            }
            assert_ne!(status, jeeves_state::TaskStatus::Failed);
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        git(root, &["ls-tree", "-r", "--name-only", "HEAD"]),
        "result.txt"
    );
    assert_eq!(git(repo.path(), &["rev-parse", "HEAD"]), main);
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT commit_sha FROM task_git_checkpoint_boundaries WHERE task_id=?")
            .bind(id)
            .fetch_all(&server.pool)
            .await
            .unwrap();
    assert!(
        rows.len() >= 2,
        "each persisted boundary must map to a commit"
    );
    assert!(rows.contains(&main));
    assert!(rows.contains(&git(root, &["rev-parse", "HEAD"])));
    assert_eq!(
        rpc(
            &server,
            "task/gitCheckpointPolicy/set",
            json!({"taskId":id,"paths":[],"enabled":false,"confirmed":true})
        )
        .await
        .unwrap()["enabled"],
        false
    );
}

#[tokio::test]
async fn a_repo_that_is_not_git_is_refused_and_repo_with_cwd_is_ambiguous() {
    let (_home, server, _model) = fixture().await;
    let plain = tempfile::tempdir().unwrap();
    let e = rpc(
        &server,
        "task/create",
        json!({"title":"x","repo":plain.path()}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("not a git repository"), "{}", e);
    let repo = repo();
    let e = rpc(
        &server,
        "task/create",
        json!({"title":"x","repo":repo.path(),"cwd":plain.path()}),
    )
    .await
    .unwrap_err();
    assert!(e.contains("not both"), "{}", e);
}

#[tokio::test]
async fn coding_task_cannot_select_another_worktree_as_its_turn_cwd() {
    let (_home, server, _model) = fixture().await;
    let repo = repo();
    let parent = rpc(
        &server,
        "task/create",
        json!({"sessionId":"jeeves-main", "repo":repo.path(),"title":"owner"}),
    )
    .await
    .unwrap();
    let sibling = super::task::create(
        &server,
        serde_json::from_value(json!({"repo": repo.path(), "title": "sibling"})).unwrap(),
    )
    .await
    .unwrap();
    let result = rpc(
        &server,
        "turn/start",
        json!({"sessionId":"jeeves-main","message":"fixture","cwd":sibling.task.cwd}),
    )
    .await;
    assert!(result.unwrap_err().contains("worktree"));
    let id = parent["task"]["id"].as_str().unwrap();
    assert_eq!(server.runs.count_by_task(id).await.unwrap(), 0);
}

#[tokio::test]
async fn a_child_waits_for_integration_before_its_parent_can_start() {
    let (_home, server, _model) = fixture().await;
    let repo = repo();
    let p = rpc(&server, "task/create", json!({"title":"parent","repo":repo.path(),"codeChecks":[{"name":"combined","after":"final","argv":["true"]}]})).await.unwrap();
    let parent = &p["task"]["id"];
    let c = rpc(
        &server,
        "task/create",
        json!({"title":"child","parentTaskId":parent}),
    )
    .await
    .unwrap();
    assert_eq!(c["task"]["parentTaskId"], *parent);
    assert_ne!(c["task"]["worktreePath"], p["task"]["worktreePath"]);
    let err = rpc(
        &server,
        "task/start",
        json!({"taskId":parent,"message":"verify"}),
    )
    .await
    .unwrap_err();
    assert!(err.contains("integration"), "{err}");
}

#[path = "coding_isolation_tests.rs"]
mod isolation;
