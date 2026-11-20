use super::{repo, rpc};
use crate::Server;
use jeeves_agent::verify::{CommandOutcome, CommandRunner};
use serde_json::{json, Value};
use std::path::Path;

async fn fixture(home: &Path, shared: &Path) -> Server {
    Server::boot(
        jeeves_state::open(&home.join("state.db")).await.unwrap(),
        jeeves_config::Settings {
            home: home.into(),
            provider: "test-unconfigured".into(),
            write_enabled: true,
            exec_enabled: true,
            exec_writable_roots: vec![shared.into()],
            exec_allow: vec!["*".into()],
            approval: "never".into(),
            ..Default::default()
        },
    )
    .await
}

#[tokio::test]
async fn ordinary_sessions_keep_shared_writes_but_coding_commands_and_checks_stay_local() {
    let (home, shared, repo) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        repo(),
    );
    let server = fixture(home.path(), shared.path()).await;
    let made = rpc(
        &server,
        "task/create",
        json!({"title":"bound","repo":repo.path()}),
    )
    .await
    .unwrap();
    let task = &made["task"];
    let root = Path::new(task["cwd"].as_str().unwrap());
    let id = task["id"].as_str().unwrap();
    let shared_file = shared.path().join("marker");
    let ordinary = server.tool_ctx("ordinary", Some(root.to_owned()), None);
    let args = json!({"path":shared_file,"content":"ordinary"});
    let result = server
        .tools
        .execute(ordinary.clone(), "write_file", args)
        .await;
    assert!(shared_file.is_file(), "{result}");
    let mut ctx = server.tool_ctx("coding", Some(root.to_owned()), None);
    ctx.worktree =
        crate::task::worktree::owned(home.path(), server.tasks.get(id).await.unwrap().as_ref());
    for (name, args) in [
        ("write_file", json!({"path":shared_file,"content":"coding"})),
        (
            "edit",
            json!({"path":shared_file,"old_string":"ordinary","new_string":"coding"}),
        ),
        (
            "apply_patch",
            json!({"patch":format!("*** Begin Patch\n*** Add File: {}\n+coding\n*** End Patch",shared_file.display())}),
        ),
    ] {
        let result = server.tools.execute(ctx.clone(), name, args).await;
        assert!(result.starts_with("Error:"), "{name}: {result}");
    }
    if !jeeves_tools::exec::sandbox_available() {
        return;
    }
    let job_cmd = format!("printf coding > '{}'", shared_file.display());
    let result = server
        .tools
        .execute(ctx.clone(), "run_command", json!({"command":job_cmd}))
        .await;
    assert!(result.starts_with("Error:"), "{result}");
    let runner = server.check_runner(Some(root), ctx.worktree.clone());
    assert!(
        matches!(runner.run(vec!["sh".into(), "-c".into(),job_cmd.clone()],root.into()).await, CommandOutcome::Ran {code,..} if code != 0)
    );
    assert_eq!(std::fs::read_to_string(&shared_file).unwrap(), "ordinary");
    server
        .runs
        .start("fixture-run", id, Some("coding"), None)
        .await
        .unwrap();
    ctx.task_run_id = Some("fixture-run".into());
    let result = server
        .tools
        .execute(
            ctx.clone(),
            "run_command",
            json!({"background":true,"command": job_cmd}),
        )
        .await;
    assert!(result.starts_with("Running in the background:"), "{result}");
    let jobs = server.jobs.as_ref().unwrap();
    let job = jobs.list("coding").pop().unwrap();
    let failed = jobs.output("coding", &job.id, 0, true).await.unwrap();
    assert_ne!(failed.exit_code, Some(0));
    let repeat = server
        .tools
        .execute(
            ctx.clone(),
            "run_command",
            json!({"background":true,"command":job_cmd}),
        )
        .await;
    assert!(repeat.contains("already stopped"), "{repeat}");
    assert_eq!(jobs.list("coding").len(), 1);

    assert_eq!(std::fs::read_to_string(&shared_file).unwrap(), "ordinary");
    let result = server
        .tools
        .execute(
            ctx,
            "run_command",
            json!({"background":true,"command":"printf fixture > result.txt"}),
        )
        .await;
    assert!(result.starts_with("Running in the background:"), "{result}");
    let job = jobs
        .list("coding")
        .into_iter()
        .find(|j| j.command.contains("result.txt"))
        .unwrap();
    let output = jobs.output("coding", &job.id, 0, true).await.unwrap();
    assert_eq!(output.exit_code, Some(0), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).unwrap(),
        "fixture"
    );
    let artifacts = server.artifacts.list(id, 10).await.unwrap();
    assert!(artifacts.iter().any(|a| a.path.ends_with(".diff")
        && std::fs::read_to_string(&a.path)
            .unwrap()
            .contains("result.txt")));
    assert!(artifacts
        .iter()
        .any(|a| a.path.ends_with(".log") && a.run_id.as_deref() == Some("fixture-run")));
    assert!(!repo.path().join("result.txt").exists());
}

async fn child(server: &Server, parent: &Value, file: &str, body: &str) -> (Value, Value) {
    let child = rpc(
        server,
        "task/create",
        json!({"title":body,"parentTaskId":parent}),
    )
    .await
    .unwrap()["task"]
        .clone();
    let root = Path::new(child["cwd"].as_str().unwrap());
    std::fs::write(root.join(file), body).unwrap();
    let checkpoint = rpc(
        server,
        "task/gitCheckpoint",
        json!({"taskId":child["id"],"paths":[file],"confirmed":true}),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agent_tasks SET status='done' WHERE id=?")
        .bind(child["id"].as_str().unwrap())
        .execute(&server.pool)
        .await
        .unwrap();
    (child["id"].clone(), checkpoint["commit"].clone())
}

#[tokio::test]
async fn parent_integrates_disjoint_checkpoints_and_leaves_conflicting_ones_for_review() {
    let (_home, server, _model) = super::fixture().await;
    let repo = repo();
    let p = rpc(&server,"task/create",json!({"repo":repo.path(),"title":"parent","codeChecks":[{"name":"combined","argv":["false"],"after":"final"}]})).await.unwrap();
    let parent = &p["task"]["id"];
    let root = Path::new(p["task"]["cwd"].as_str().unwrap());
    let a = child(&server, parent, "left", "one").await;
    let b = child(&server, parent, "right", "two").await;
    let c = child(&server, parent, "left", "conflict").await;
    let params = |(id, sha): &(Value, Value)| json!({"parentTaskId":parent,"childTaskId":id,"commit":sha,"confirmed":true});
    for part in [&a, &b, &b] {
        rpc(&server, "task/integrate", params(part)).await.unwrap();
    }
    let err = rpc(&server, "task/integrate", params(&c))
        .await
        .unwrap_err();
    assert!(err.contains("resolve"), "{err}");
    assert_eq!(std::fs::read_to_string(root.join("left")).unwrap(), "one");
    assert_eq!(std::fs::read_to_string(root.join("right")).unwrap(), "two");
    let events = server
        .events
        .list(parent.as_str().unwrap(), 20)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "child_integrated")
            .count(),
        2
    );
    assert!(!repo.path().join("left").exists());
    let view = rpc(&server, "task/get", json!({"taskId":parent}))
        .await
        .unwrap();
    assert_eq!(view["task"]["status"], "pending");
}

#[tokio::test]
async fn parent_final_command_measures_the_integrated_files() {
    use jeeves_provider::{mock::Mock, Resolved};
    if !jeeves_tools::exec::sandbox_available() {
        return;
    }
    let (home, shared, repo) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        repo(),
    );
    let server = fixture(home.path(), shared.path()).await;
    server.install_provider(Resolved {
        client: std::sync::Arc::new(Mock::new(vec![Mock::text("complete"); 12])),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    for (required, expected) in [("both", "done"), ("missing", "needs_attention")] {
        let p = rpc(&server,"task/create",json!({"repo":repo.path(),"title":"verify","maxRounds":1,"codeChecks":[{"name":"combined","argv":["test","-f",required],"after":"final"}]})).await.unwrap();
        let parent = &p["task"]["id"];
        let (id, commit) = child(&server, parent, "both", "fixture").await;
        rpc(
            &server,
            "task/integrate",
            json!({"parentTaskId":parent,"childTaskId":id,"commit":commit,"confirmed":true}),
        )
        .await
        .unwrap();
        rpc(
            &server,
            "task/start",
            json!({"taskId":parent,"message":"Verify."}),
        )
        .await
        .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(12), async {
            loop {
                let view = rpc(&server, "task/get", json!({"taskId":parent}))
                    .await
                    .unwrap();
                let status = view["task"]["status"].as_str().unwrap();
                if !matches!(status, "pending" | "running") {
                    assert_eq!(status, expected, "{}", view["task"]);
                    assert_eq!(view["task"]["checks"][0]["passed"], expected == "done");
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn independent_worktrees_hold_simultaneous_writer_permits() {
    use jeeves_tools::resources::ResourceCoordinator;
    let gate = crate::resources::Mutations::default();
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = gate.acquire("write_file", Some(a.path())).await.unwrap();
    let second = gate.acquire("run_command", Some(b.path())).await.unwrap();
    let same = gate.acquire("edit", Some(a.path()));
    let shared = gate.acquire("write_file", None);
    for blocked in [same, shared] {
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(30), blocked)
                .await
                .is_err()
        );
    }
    drop((first, second));
    assert!(gate.acquire("edit", None).await.is_ok());
}

#[tokio::test]
async fn dispatched_task_applies_its_stored_scope_to_model_write_tools() {
    use jeeves_provider::{mock::Mock, Resolved};
    let (home, shared, repo) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        repo(),
    );
    let server = fixture(home.path(), shared.path()).await;
    let outside = shared.path().join("outside.txt");
    server.install_provider(Resolved {
        client: std::sync::Arc::new(Mock::new(vec![
            Mock::tool_use(
                "outside",
                "write_file",
                json!({"path":outside,"content":"outside"}),
            ),
            Mock::tool_use(
                "inside",
                "write_file",
                json!({"path":"inside.txt","content":"inside"}),
            ),
            Mock::text("finished"),
        ])),
        model: "fixture".into(),
        small_model: "fixture".into(),
        provider_id: "mock".into(),
        vision: false,
        context_window: 100_000,
    });
    let made = rpc(
        &server,
        "task/create",
        json!({"title":"scope","repo":repo.path()}),
    )
    .await
    .unwrap();
    let task = &made["task"];
    rpc(
        &server,
        "task/start",
        json!({"taskId":task["id"],"message":"Write the fixture."}),
    )
    .await
    .unwrap();
    let root = Path::new(task["cwd"].as_str().unwrap());
    tokio::time::timeout(std::time::Duration::from_secs(12), async {
        while !root.join("inside.txt").is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    assert!(!outside.exists());
}
