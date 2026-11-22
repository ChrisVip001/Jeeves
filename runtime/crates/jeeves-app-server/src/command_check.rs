//! `Acceptance::CommandSucceeds` on the host: runs through the same exec gate and sandbox as
//! `run_command`. Exec off, no sandbox, a refusal by the allow table or an unknown outcome all
//! come back as `Skipped`, never as a pass. Output is kept in the task workspace.

use jeeves_agent::verify::{CommandFut, CommandOutcome, CommandRunner};
use std::path::PathBuf;

pub(crate) struct ExecRunner {
    server: crate::Server,
    ctx: jeeves_tools::ToolCtx,
    pub(crate) policy: Option<jeeves_tools::exec::ExecPolicy>,
    /// Where the output of each check is written; `None` = not kept.
    pub(crate) log_dir: Option<PathBuf>,
}

/// POSIX single-quote one word.
fn quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%".contains(c))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

pub(crate) fn shell_line(argv: &[String]) -> String {
    argv.iter().map(|w| quote(w)).collect::<Vec<_>>().join(" ")
}

impl CommandRunner for ExecRunner {
    fn run(&self, argv: Vec<String>, cwd: PathBuf) -> CommandFut {
        let policy = self.policy.clone();
        let server = self.server.clone();
        let mut ctx = self.ctx.clone();
        let log_dir = self.log_dir.clone();
        Box::pin(async move {
            let Some(policy) = policy else {
                return CommandOutcome::Skipped("exec off".into());
            };

            let command = format!(
                "cd {} && {}",
                quote(&cwd.display().to_string()),
                shell_line(&argv)
            );
            ctx.worktree = policy.worktree.clone();
            if let Err(reason) = server
                .tools
                .admit_call(
                    &mut ctx,
                    "run_command",
                    &serde_json::json!({"command":command}),
                )
                .await
            {
                return CommandOutcome::Skipped(reason);
            }
            let (code, text) = match jeeves_tools::exec::execute_process(
                &command,
                &policy,
                &[cwd],
                ctx.approval.as_deref(),
                None,
                ctx.on_uncertain,
            )
            .await
            {
                Ok(measured) => measured,
                Err(reason) => return CommandOutcome::Skipped(reason.to_string()),
            };
            let outcome = CommandOutcome::Ran {
                code,
                output: text.clone(),
            };
            if let (Some(dir), CommandOutcome::Ran { .. }) = (log_dir, &outcome) {
                let name = format!("check-{:016x}.log", fnv(&argv.join("\0")));
                if let Some(root) = &policy.worktree {
                    if let Err(error) = jeeves_tools::roots::check_worktree(root, &dir.join(&name))
                    {
                        eprintln!("(jeeves) check output was not kept: {error}");
                        return outcome;
                    }
                }
                if std::fs::create_dir_all(&dir).is_ok() {
                    if let Err(e) = std::fs::write(dir.join(&name), &text) {
                        eprintln!("(jeeves) check output was not kept ({name}): {e}");
                    }
                }
            }
            outcome
        })
    }
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

impl crate::Server {
    /// The runner for one verification. `None` policy when exec is off.
    pub(crate) fn check_runner(
        &self,
        ctx: jeeves_tools::ToolCtx,
        root: Option<&std::path::Path>,
        worktree: Option<PathBuf>,
    ) -> std::sync::Arc<ExecRunner> {
        let settings = self.settings();
        let policy = settings.exec_enabled.then(|| {
            let mut policy = crate::server::tools::exec_policy(
                &settings,
                self.exec_allow.clone(),
                self.exec_session_allow.clone(),
            );
            policy.worktree = worktree;
            policy
        });
        std::sync::Arc::new(ExecRunner {
            ctx,
            server: self.clone(),
            policy,
            log_dir: root.map(|r| r.join("checks")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_is_quoted_never_interpreted() {
        let line = shell_line(&[
            "echo".into(),
            "a b".into(),
            "x;rm -rf /".into(),
            "it's".into(),
        ]);
        assert_eq!(line, "echo 'a b' 'x;rm -rf /' 'it'\\''s'");
    }
}
