//! Capture a bounded, local review artifact while the job still holds its worktree permit.
use jeeves_tools::exec::{ExecPolicy, scrub, spawn_sandboxed};
use std::path::Path;
use tokio::io::AsyncReadExt;

pub(super) async fn evidence(
    policy: &ExecPolicy,
    root: &Path,
    log: &Path,
) -> Result<Vec<(String, i64)>, String> {
    const LIMIT: u64 = 128 * 1024;
    let mut diagnostic = policy.clone();
    diagnostic.network = false;
    diagnostic.secrets = None;
    let cmd = "export GIT_OPTIONAL_LOCKS=0; printf 'Untracked paths:\\n' && git -c core.fsmonitor=false ls-files --others --exclude-standard && printf '\\nTracked changes against HEAD:\\n' && git -c core.fsmonitor=false diff --no-ext-diff --no-textconv --no-color HEAD";
    let mut child = spawn_sandboxed(cmd, &diagnostic, &[root.to_path_buf()])?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .expect("piped stdout")
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .await?;
        if bytes.len() as u64 > LIMIT {
            child.kill().await?;
            bytes.truncate(LIMIT as usize);
            bytes.extend_from_slice(b"\n[worktree diff truncated]\n");
        } else if !child.wait().await?.success() {
            let mut message = String::new();
            (&mut stderr)
                .take(2048)
                .read_to_string(&mut message)
                .await?;
            return Err(std::io::Error::other(format!(
                "Git could not capture the worktree diff: {}",
                scrub(message, policy).trim()
            )));
        }
        Ok::<_, std::io::Error>(bytes)
    })
    .await
    .map_err(|_| "worktree diff timed out".to_string())?
    .map_err(|e| e.to_string())?;
    let diff = log.with_extension("diff");
    let body = scrub(String::from_utf8_lossy_owned(output), policy);
    tokio::fs::write(&diff, &body)
        .await
        .map_err(|e| e.to_string())?;
    let bytes = tokio::fs::metadata(log)
        .await
        .map_err(|e| e.to_string())?
        .len();
    Ok(vec![
        (log.display().to_string(), bytes as i64),
        (diff.display().to_string(), body.len() as i64),
    ])
}
