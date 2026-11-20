//! `Acceptance::CommandSucceeds` on the host: runs through the same exec gate and sandbox as
//! `run_command`. Exec off, no sandbox, a refusal by the allow table or an unknown outcome all
//! come back as `Skipped`, never as a pass. Output is kept in the task workspace.

use jeeves_agent::verify::{CommandFut, CommandOutcome, CommandRunner};
use std::path::PathBuf;

pub(crate) struct ExecRunner {
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

/// Turn `execute`'s text into an outcome. `exit code N` / `Error: exit code N` ran; any other
/// `Error:` is a refusal or an unknown outcome and counts as not run.
pub(crate) fn outcome_of(text: &str) -> CommandOutcome {
    let first = text.lines().next().unwrap_or("");
    let code = first
        .strip_prefix("Error: exit code ")
        .or_else(|| first.strip_prefix("exit code "))
        .and_then(|n| n.trim().parse::<i32>().ok());
    match code {
        Some(code) => CommandOutcome::Ran {
            code,
            output: text.to_string(),
        },
        None => CommandOutcome::Skipped(first.trim_start_matches("Error: ").to_string()),
    }
}

impl CommandRunner for ExecRunner {
    fn run(&self, argv: Vec<String>, cwd: PathBuf) -> CommandFut {
        let policy = self.policy.clone();
        let log_dir = self.log_dir.clone();
        Box::pin(async move {
            let Some(mut policy) = policy else {
                return CommandOutcome::Skipped("exec off".into());
            };
            policy.approval = false;
            let command = format!(
                "cd {} && {}",
                quote(&cwd.display().to_string()),
                shell_line(&argv)
            );
            let text = jeeves_tools::exec::execute(&command, &policy, &[cwd], None, None).await;
            let outcome = outcome_of(&text);
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

    #[test]
    fn exit_text_maps_to_outcomes() {
        assert!(matches!(
            outcome_of("exit code 0\nok"),
            CommandOutcome::Ran { code: 0, .. }
        ));
        assert!(matches!(
            outcome_of("Error: exit code 1\nbad"),
            CommandOutcome::Ran { code: 1, .. }
        ));
        assert_eq!(
            outcome_of("Error: nothing is allowed"),
            CommandOutcome::Skipped("nothing is allowed".into())
        );
        assert!(matches!(
            outcome_of("Error: The command exceeded 5 seconds and was killed; outcome is unknown."),
            CommandOutcome::Skipped(_)
        ));
    }
}
