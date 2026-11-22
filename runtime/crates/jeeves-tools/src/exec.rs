//! `run_command` — lets Jeeves run commands on this machine, but every gate is visible.
//!
//! This is the highest-privilege thing in the project, so its shape is "off by default +
//! three gates + an OS-level sandbox":
//!
//! 1. **Hard-deny list**: catastrophic things never run no matter how it's configured
//!    (`sudo`, `mkfs`, fork bombs…). Not configurable — a fuse config can switch off isn't a fuse.
//! 2. **allowlist**: nothing runs unless a rule in `JEEVES_EXEC_ALLOW` matches. An empty
//!    allowlist = nothing may run (deny by default, not allow by default).
//! 3. **Sandbox**: the command runs inside macOS `sandbox-exec` / Linux `bubblewrap`,
//!    writes limited to the working directory and temp dirs. **No sandbox, no run** —
//!    never run a command bare "just to make it work"; that's why this module exists.
//!
//! Like the rest of the tool layer: refusals and failures are **text**, not errors. A refusal
//! must say why (which rule blocked it, how to relax it) so the model can explain to the user.
//!
//! ## Boundaries (written honestly here; don't let them quietly grow)
//!
//! * **Network off by default** (`JEEVES_EXEC_NETWORK=1` to allow): the sandbox really cuts it
//!   (seatbelt's `(deny network*)` / bwrap's `--unshare-net`). A behavior change: earlier
//!   versions only restricted write paths, and commands could still reach the network.
//! * Writable roots are **the workspace + `JEEVES_EXEC_WRITABLE_ROOTS`, nothing else** — the
//!   same definition `write_file`/`edit` use (see `crate::roots`). `<home>` is not a root:
//!   it holds the control plane (`settings.json` = the allow table, `hooks.json` = commands
//!   on lifecycle events), so a shell that could write it could grant itself more than it
//!   had. Build caches are why `JEEVES_EXEC_WRITABLE_ROOTS` exists; the refusal text names it.
//! * Hard-deny is **substring matching**: fancy evasions slip past; allowlist and sandbox are the real defense.
//! * No interactive approval popup (the third gate is expressed by the allowlist).
//! * Config is read once at startup (`config/write` changes need a process restart).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

use crate::approval::RememberTarget;
use crate::{require_str, Tool, ToolCtx};

/// Output cap. Past it, truncate and **label honestly** — a `find /` can flood the context,
/// and the model must know "this isn't everything".
const MAX_OUTPUT_CHARS: usize = 8_000;

/// Push a delta once this much unshipped output has piled up, regardless of time.
const DELTA_MAX_CHARS: usize = 16 * 1024;

/// And once this long a while has passed with the pipe quiet: a compile prints a line every
/// few seconds and the human should not stare at nothing.
const DELTA_INTERVAL: Duration = Duration::from_millis(250);

/// The execution policy fixed at startup (from `JEEVES_EXEC*`).
#[derive(Debug, Clone, Default)]
pub struct ExecPolicy {
    /// Rules from `JEEVES_EXEC_ALLOW`. Empty = nothing may run. A shared cell: replacing the table
    /// at runtime (`config/write`) must revoke rules for the very next call, not after a restart.
    pub allow: Arc<Mutex<Vec<String>>>,
    pub timeout_secs: i64,
    /// Whether the sandbox may use the network (`JEEVES_EXEC_NETWORK`). Default **false**: an
    /// allowed command shouldn't default to being able to send data out. Commands like
    /// `cargo test` that download must turn the flag on explicitly.
    pub network: bool,
    /// When the allowlist doesn't match, may we **ask once** (`JEEVES_APPROVAL=on-request`)?
    /// Default false = deny outright (same as the old behavior).
    pub approval: bool,
    /// Seconds to wait for a human answer (`JEEVES_APPROVAL_TIMEOUT`).
    pub approval_timeout_secs: i64,
    /// Where oversized output is spilled (`<home>/spill`). `None` = don't spill, just truncate.
    ///
    /// Truncation is required (a `find /` can flood the context), but **what's dropped is gone**.
    /// Spilling the original keeps it reachable for model and human alike.
    pub spill_dir: Option<PathBuf>,
    /// Host-assigned task root. Isolated commands receive no shared writable directories.
    pub worktree: Option<PathBuf>,
    /// Extra writable roots (`JEEVES_EXEC_WRITABLE_ROOTS`): beyond the working dir.
    /// Build caches are the classic case — without it, `cargo build` can't write `~/.cargo`
    /// in the sandbox. Entries that would hand back the control plane are dropped by
    /// `crate::roots` (see [`spawn_sandboxed`]).
    pub extra_roots: Vec<PathBuf>,
    /// Jeeves's home, carried so the sandbox can keep the **control plane** out of the writable
    /// set (`crate::roots::HOME_CONTROL`). Empty = only the disk root is refused; a real
    /// server always fills it in.
    pub home: PathBuf,
    /// Commands a human already said yes to **in this process** ("always allow in this session"). Exact match,
    /// no wildcards. Shared, so the very next call sees the answer — an approval that only takes
    /// effect after a restart is not what the click promised. Never written to disk (that is
    /// `settings.json`'s `exec_allow`, the "remember this one" answer).
    pub session_allow: Arc<Mutex<Vec<String>>>,
    /// The credential vault: `(NAME, value)` pairs, asked for at spawn time. Each becomes
    /// `JEEVES_SECRET_<NAME>` in that one process, and `scrub` hides the value from the output, so the
    /// model can *use* a credential without ever reading it. `None` = no vault.
    pub secrets: Option<SecretsFn>,
}

/// Where a policy gets the vault's values. A named type so `ExecPolicy` keeps `Debug`.
#[derive(Clone)]
pub struct SecretsFn(pub Arc<dyn Fn() -> Vec<(String, String)> + Send + Sync>);

impl std::fmt::Debug for SecretsFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretsFn(..)")
    }
}

impl SecretsFn {
    fn get(&self) -> Vec<(String, String)> {
        (self.0)()
    }
}

/// Replace every vault value in `text` with `«NAME»`. Values shorter than 4 characters are skipped:
/// replacing "1" would mangle ordinary output and hides nothing worth hiding.
pub fn scrub(text: String, policy: &ExecPolicy) -> String {
    let Some(secrets) = &policy.secrets else {
        return text;
    };
    let mut text = text;
    for (name, value) in secrets.get() {
        if value.chars().count() >= 4 {
            text = text.replace(&value, &format!("«{name}»"));
        }
    }
    text
}

/// Patterns that never run. Lowercased comparison, substring match — better to miss a
/// fancy evasion than to build a regex that hits normal commands.
const HARD_DENY: &[&str] = &[
    "sudo ",
    "sudo\t",
    "doas ",
    "mkfs",
    "dd if=",
    "dd of=",
    "shutdown",
    "reboot",
    "halt ",
    ":(){",
    "chmod -r 777 /",
    "chmod 777 /",
    "chown -r /",
    "> /dev/disk",
    "> /dev/sd",
    "diskutil erase",
    "launchctl unload",
    "systemctl disable",
    "rm -rf /*",
    "rm -rf ~",
    "rm -rf $home",
    "eval $(",
    "history -c",
];

/// Hand-rolled detection of "download piped straight into a shell": `curl … | sh` has a URL
/// in the middle, so literal substring matching can't see it — yet it's the classic "ran a
/// stranger's code". Split the pipe; check downloader on the left, shell on the right.
fn pipes_into_a_shell(command: &str) -> bool {
    let segments: Vec<&str> = command.split('|').map(str::trim).collect();
    segments.windows(2).any(|pair| {
        let downloads = pair[0].starts_with("curl") || pair[0].starts_with("wget");
        let runs = matches!(
            pair[1].split_whitespace().next(),
            Some("sh" | "bash" | "zsh" | "dash" | "python" | "python3" | "perl" | "ruby")
        );
        downloads && runs
    })
}

/// The three outcomes a gate can produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Let it through.
    Allow,
    /// Deny. The reason goes straight to the model (handed over with an `Error:` prefix).
    Deny(String),
    /// No allowlist match, but `JEEVES_APPROVAL=on-request` is on: ask once, then decide.
    ///
    /// **Only the allowlist gate may be asked about.** The hard-deny list and the sandbox
    /// returned before this — approval never touches them; a fuse a click can bypass isn't a fuse.
    NeedsApproval { reason: String },
}

/// Should the command run? Three gates in order; the first to stop it decides.
pub fn vet(command: &str, policy: &ExecPolicy) -> Gate {
    let lower = command.to_lowercase();

    if let Some(hit) = HARD_DENY.iter().find(|pattern| lower.contains(*pattern)) {
        return Gate::Deny(format!(
            "This command was refused by the hard blocklist (matched '{hit}') — no configuration will let it run."
        ));
    }
    if pipes_into_a_shell(&lower) {
        return Gate::Deny(
            "This command was refused by the hard blocklist (piping downloaded content straight into a shell) — no configuration will let it run."
                .to_string(),
        );
    }

    let session_rules: Vec<String> = match policy.session_allow.lock() {
        Ok(list) => list.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let allow: Vec<String> = match policy.allow.lock() {
        Ok(list) => list.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    let matched = allow.iter().any(|rule| matches_rule(rule, command))
        || session_rules
            .iter()
            .any(|remembered| remembered.trim() == command.trim());
    if !matched {
        let why = if allow.is_empty() && session_rules.is_empty() {
            "The exec tool has no command allowed. To allow a class of commands, set JEEVES_EXEC_ALLOW, e.g. JEEVES_EXEC_ALLOW='cargo test,git status,ls *'."
                .to_string()
        } else {
            let mut known: Vec<String> = allow.clone();
            known.extend(session_rules.iter().map(|c| format!("{c} (this session)")));
            format!(
                "No allow rule matched. Current rules: {}. To allow it, add a rule to JEEVES_EXEC_ALLOW (a trailing * is accepted).",
                known.join(", ")
            )
        };
        return if policy.approval {
            Gate::NeedsApproval { reason: why }
        } else {
            Gate::Deny(why)
        };
    }

    if !sandbox_available() {
        return Gate::Deny(
            "This machine has no usable sandbox (macOS needs sandbox-exec, Linux needs bubblewrap); refusing to run commands outside a sandbox."
                .to_string(),
        );
    }
    Gate::Allow
}

/// Allow-rule matching: exact string equality, or prefix match when the rule ends in `*`;
/// a lone `*` means "allow everything" (it still can't pass the hard-deny list).
fn matches_rule(rule: &str, command: &str) -> bool {
    let rule = rule.trim();
    if rule == "*" {
        return true;
    }
    match rule.strip_suffix('*') {
        Some(prefix) => command.starts_with(prefix.trim_end_matches(' ')),
        None => command.trim() == rule,
    }
}

/// Is the sandbox usable right now?
///
/// "Usable" is **tested**, not looked up: the file being there doesn't mean it runs.
/// This matters most on Linux — Ubuntu 24.04 restricts unprivileged user namespaces with
/// AppArmor by default, so `bwrap` installed there still fails with "setting up uid map:
/// Permission denied". Allow by only checking "is bwrap in PATH" and the command runs with
/// the sandbox **not in effect** — the worst thing this module could do.
pub fn sandbox_available() -> bool {
    sandbox_backend().is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sandbox {
    /// macOS: `sandbox-exec -p <profile> <shell> -c <command>`.
    Seatbelt,
    /// Linux: `bwrap … <shell> -c <command>`.
    Bubblewrap,
}

fn sandbox_backend() -> Option<Sandbox> {
    static BACKEND: std::sync::OnceLock<Option<Sandbox>> = std::sync::OnceLock::new();
    *BACKEND.get_or_init(|| {
        if Path::new("/usr/bin/sandbox-exec").exists() {
            return Some(Sandbox::Seatbelt);
        }
        if which("bwrap") && bubblewrap_runs() {
            return Some(Sandbox::Bubblewrap);
        }
        None
    })
}

/// Actually launch bubblewrap once (`/bin/true`) and see whether it can build a sandbox.
fn bubblewrap_runs() -> bool {
    std::process::Command::new("bwrap")
        .args([
            "--ro-bind",
            "/",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--bind",
            "/tmp",
            "/tmp",
            "--die-with-parent",
            "--",
            "/bin/true",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

/// Keep the command's write access inside these `writable` roots (plus temp dirs).
///
/// Roots must be **canonicalized** before going into sandbox rules: on macOS `/var/folders/…`
/// is a symlink to `/private/var/folders/…`, and seatbelt wants real paths — rules written
/// with the former silently fail to allow the directory.
/// The sandbox command's argv.
///
/// Pulled out to be **testable**: the argument shape (especially "is the network cut")
/// shouldn't only be visible when actually run — such a "test" tests nothing.
pub(crate) fn sandbox_argv(
    backend: Sandbox,
    command: &str,
    writable: &[PathBuf],
    network: bool,
    isolated: bool,
) -> Vec<String> {
    let roots: Vec<PathBuf> = writable
        .iter()
        .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
        .collect();
    match backend {
        Sandbox::Seatbelt => {
            let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n");
            for root in &roots {
                profile.push_str(&format!(
                    "(allow file-write* (subpath {}))\n",
                    serde_json::Value::String(root.to_string_lossy().into_owned())
                ));
            }
            if isolated {
                profile.push_str("(allow file-write-data (literal \"/dev/null\"))\n");
            } else {
                for tmp in ["/tmp", "/private/tmp"] {
                    profile.push_str(&format!("(allow file-write* (subpath \"{tmp}\"))\n"));
                }
                profile.push_str(&format!(
                    "(allow file-write* (subpath \"{}\"))\n",
                    std::env::temp_dir().display()
                ));
            }
            if !network {
                profile.push_str("(deny network*)\n");
            }
            vec![
                "/usr/bin/sandbox-exec".to_string(),
                "-p".to_string(),
                profile,
                "/bin/sh".to_string(),
                "-c".to_string(),
                command.to_string(),
            ]
        }
        Sandbox::Bubblewrap => {
            let mut args = vec![
                "--ro-bind".to_string(),
                "/".to_string(),
                "/".to_string(),
                "--dev".to_string(),
                "/dev".to_string(),
                "--proc".to_string(),
                "/proc".to_string(),
                "--die-with-parent".to_string(),
            ];
            if !isolated {
                args.extend(["--bind", "/tmp", "/tmp"].map(String::from));
            }
            for root in &roots {
                args.push("--bind".to_string());
                args.push(root.display().to_string());
                args.push(root.display().to_string());
            }
            if !network {
                args.push("--unshare-net".to_string());
            }
            args.push("--".to_string());
            args.push("/bin/sh".to_string());
            args.push("-c".to_string());
            args.push(command.to_string());
            let mut argv = vec!["bwrap".to_string()];
            argv.extend(args);
            argv
        }
    }
}

/// Gate + approval. `Ok(())` = may run; `Err(reason)` = don't (reason is model-readable).
///
/// **Foreground and background jobs share this spot**: a background command that could
/// bypass approval makes it theater — "background it, run at leisure" is the natural bypass.
pub async fn pass_gate(
    command: &str,
    policy: &ExecPolicy,
    approval: Option<&dyn crate::approval::ApprovalBroker>,
) -> Result<(), String> {
    match vet(command, policy) {
        Gate::Deny(why) => Err(why),
        Gate::Allow => Ok(()),
        Gate::NeedsApproval { reason } => {
            let Some(broker) = approval else {
                return Err(format!(
                    "{reason}\n(nor is anyone there to ask: with JEEVES_APPROVAL=on-request, only a prompt from the terminal or the cockpit has somebody to answer it.)"
                ));
            };
            let request = crate::approval::ApprovalRequest {
                tool: "run_command".to_string(),
                args_preview: command.to_string(),
                reason,
                timeout_secs: policy.approval_timeout_secs,
                remember: Some(RememberTarget::Command(command.trim().to_string())),
                action: None,
            };
            if broker.request(request).await {
                Ok(())
            } else {
                Err("This run was not approved (or nobody answered); skipped.".to_string())
            }
        }
    }
}

/// Turn the policy into a command that can be **spawned directly**: sandbox args, writable-root
/// merge, and the network switch all in one spot, shared by foreground and background jobs —
/// write the sandbox args twice and one path will sooner or later forget `--unshare-net`.
///
/// The writable set is filtered **here**, not at the call sites: `--bind /` and a home that
/// holds the allow table are both one typo away, and a caller that forgot to filter would
/// otherwise hand the sandbox a way to rewrite its own permissions.
pub fn spawn_sandboxed(
    command: &str,
    policy: &ExecPolicy,
    writable: &[PathBuf],
) -> Result<tokio::process::Child, String> {
    spawn(command, policy, writable, false)
}

/// A background coding process keeps a pipe to its runtime. EOF covers both explicit
/// cancellation and abrupt runtime exit, when a Rust drop handler cannot run.
pub fn spawn_background_sandboxed(
    command: &str,
    policy: &ExecPolicy,
    writable: &[PathBuf],
) -> Result<tokio::process::Child, String> {
    spawn(command, policy, writable, policy.worktree.is_some())
}

fn spawn(
    command: &str,
    policy: &ExecPolicy,
    writable: &[PathBuf],
    supervised: bool,
) -> Result<tokio::process::Child, String> {
    let backend =
        sandbox_backend().ok_or_else(|| "This machine has no usable sandbox".to_string())?;

    let isolated = policy.worktree.as_deref();
    let mut roots = Vec::new();
    if let Some(root) = isolated {
        let [cwd] = writable else {
            return Err("an isolated command needs its task working directory".into());
        };
        crate::roots::check_worktree(root, cwd)?;
        roots.push(root.to_owned());
    } else {
        roots = crate::roots::resolve_roots(writable, &policy.extra_roots, &policy.home).workspaces;
    }
    let argv = sandbox_argv(backend, command, &roots, policy.network, isolated.is_some());
    let mut command_builder = if supervised {
        const SUPERVISE: &str = r#"
exec 3<&0
( IFS= read -r line <&3; kill -KILL 0 ) &
watch=$!
"$@" 3<&- </dev/null &
worker=$!
wait "$worker"
status=$?
kill "$watch" 2>/dev/null
wait "$watch" 2>/dev/null
exit "$status"
"#;
        let mut guard = tokio::process::Command::new("/bin/sh");
        #[cfg(unix)]
        guard.process_group(0);
        guard.args(["-c", SUPERVISE, "jeeves-job"]).args(&argv);
        guard.stdin(Stdio::piped());
        guard
    } else {
        let mut direct = tokio::process::Command::new(&argv[0]);
        direct.args(&argv[1..]).stdin(Stdio::null());
        direct
    };
    if let Some(root) = isolated {
        command_builder
            .current_dir(&writable[0])
            .env("TMPDIR", root);
    }
    if let Some(secrets) = &policy.secrets {
        for (name, value) in secrets.get() {
            command_builder.env(format!("JEEVES_SECRET_{name}"), value);
        }
    }
    command_builder
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("The command would not start: {e}"))
}

/// Read stdout in chunks, pushing deltas as they arrive and collecting the full text.
///
/// Takes over `read_to_string` so a long build shows progress. Chunk boundaries cut UTF-8
/// characters in half, so the tail that does not decode yet is carried to the next chunk —
/// decoding each read independently would scatter U+FFFD through Chinese output.
async fn read_stdout(
    stdout: &mut tokio::process::ChildStdout,
    out: &mut String,
    sink: Option<&crate::DeltaSink>,
) -> std::io::Result<usize> {
    let mut buf = [0u8; 4096];
    let mut carry: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut pending = String::new();
    let mut last_emit = std::time::Instant::now();

    loop {
        let read = stdout.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        total += read;
        carry.extend_from_slice(&buf[..read]);

        let (text, consumed) = match std::str::from_utf8(&carry) {
            Ok(text) => (text.to_string(), text.len()),
            Err(invalid) => match invalid.error_len() {
                Some(len) => {
                    let upto = invalid.valid_up_to();
                    let mut text = String::from_utf8_lossy(&carry[..upto]).to_string();
                    text.push('\u{FFFD}');
                    (text, upto + len)
                }
                None => (
                    String::from_utf8_lossy(&carry[..invalid.valid_up_to()]).to_string(),
                    invalid.valid_up_to(),
                ),
            },
        };
        if consumed == 0 {
            continue;
        }
        carry.drain(..consumed);
        out.push_str(&text);

        if let Some(sink) = sink {
            pending.push_str(&text);
            if pending.len() >= DELTA_MAX_CHARS || last_emit.elapsed() >= DELTA_INTERVAL {
                sink(&pending);
                pending.clear();
                last_emit = std::time::Instant::now();
            }
        }
    }

    if !carry.is_empty() {
        let text = String::from_utf8_lossy(&carry).to_string();
        out.push_str(&text);
        pending.push_str(&text);
    }
    if let Some(sink) = sink {
        if !pending.is_empty() {
            sink(&pending);
        }
    }
    Ok(total)
}

/// Run a command. **Never returns Err** — refusals, timeouts, non-zero exits are all text.
///
/// `writable` holds the roots the caller decides **dynamically** (the turn's working dir);
/// `policy.extra_roots` (from config) is merged in by [`spawn_sandboxed`], which also filters
/// the result (see `crate::roots`) — merging inside keeps "what the policy says" true on
/// every call path: scatter the merge across callers and one path will silently ignore it.
///
/// `on_delta` (optional) receives stdout **while the command is still running**. It is a
/// progress feed, not a result: the return value below remains the single source of truth, so
/// a dropped chunk only costs a repaint.
pub async fn execute(
    command: &str,
    policy: &ExecPolicy,
    writable: &[PathBuf],
    approval: Option<&dyn crate::approval::ApprovalBroker>,
    on_delta: Option<crate::DeltaSink>,
) -> String {
    execute_with_notice(command, policy, writable, approval, on_delta, None).await
}

pub(crate) async fn execute_with_notice(
    command: &str,
    policy: &ExecPolicy,
    writable: &[PathBuf],
    approval: Option<&dyn crate::approval::ApprovalBroker>,
    on_delta: Option<crate::DeltaSink>,
    on_uncertain: Option<crate::UncertainSink>,
) -> String {
    let unknown = |reason: String| {
        if let Some(notice) = &on_uncertain {
            notice(&reason);
        }
        format!("Error: {reason}; outcome is unknown. Reconcile any committed effects before continuing.")
    };
    if let Err(why) = pass_gate(command, policy, approval).await {
        return format!("Error: {why}");
    }
    let mut child = match spawn_sandboxed(command, policy, writable) {
        Ok(child) => child,
        Err(why) => return format!("Error: {why}"),
    };

    let timeout = Duration::from_secs(policy.timeout_secs.max(1) as u64);
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");

    let collected = tokio::time::timeout(timeout, async {
        let mut out = String::new();
        let mut err = String::new();
        let (out_result, err_result) = tokio::join!(
            read_stdout(&mut stdout, &mut out, on_delta.as_ref()),
            stderr.read_to_string(&mut err)
        );
        let status = child.wait().await;
        (out_result, err_result, status, out, err)
    })
    .await;

    let (out_result, err_result, status, out, err) = match collected {
        Ok(collected) => collected,
        Err(_) => {
            return unknown(format!(
                "The command exceeded {} seconds and was killed",
                policy.timeout_secs
            ));
        }
    };
    if let Err(e) = out_result {
        return unknown(format!("Could not read command output: {e}"));
    }
    if let Err(e) = err_result {
        return unknown(format!("Could not read command standard error: {e}"));
    }
    let status = match status {
        Ok(status) => status,
        Err(e) => return unknown(format!("Could not collect command status: {e}")),
    };
    let Some(code) = status.code() else {
        return unknown("Command terminated without an exit code".into());
    };

    let mut combined = if code == 0 {
        format!("exit code {code}\n{out}")
    } else {
        format!("Error: exit code {code}\n{out}")
    };
    if !err.trim().is_empty() {
        combined.push_str("\n--- stderr ---\n");
        combined.push_str(&err);
    }
    let combined = scrub(combined, policy);
    match crate::spill::stub(
        policy.spill_dir.as_deref(),
        &combined,
        MAX_OUTPUT_CHARS,
        "command",
    ) {
        Some(stub) => stub.text,
        None => combined,
    }
}

/// Clean files older than `max_age_days` from `spill/`. Runs once at startup.
///
/// Time-based only, no quota (see docs/limitations.md): a one-shot sweep is more predictable
/// than a capacity guess. Failures don't error — this is housekeeping, not a feature.
pub fn prune_spill(home: &Path, max_age_days: u64) {
    let spill = home.join("spill");
    let Ok(days) = std::fs::read_dir(&spill) else {
        return;
    };
    let cutoff =
        std::time::SystemTime::now() - std::time::Duration::from_secs(max_age_days * 24 * 60 * 60);
    for day in days.flatten() {
        let path = day.path();
        if !path.is_dir() {
            continue;
        }
        let fresh = std::fs::read_dir(&path)
            .map(|files| {
                files.flatten().any(|file| {
                    file.metadata()
                        .and_then(|meta| meta.modified())
                        .map(|modified| modified > cutoff)
                        .unwrap_or(true)
                })
            })
            .unwrap_or(true);
        if !fresh {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// The `run_command` registered into the tool table. Only registered when `JEEVES_EXEC=1` —
/// when it's off, the model can't even see this tool.
pub fn run_command(policy: ExecPolicy) -> Tool {
    let policy = Arc::new(policy);
    Tool {
        name: "run_command".to_string(),
        description: "Run a shell command on the user's machine (sandboxed: it may only write the working directory and the temp directory). Good for checking state, running tests, read-only git. A refused command comes back with the reason — adjust to it, or ask the user to widen JEEVES_EXEC_ALLOW."
            .to_string(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The shell command to run"},
                "cwd": {"type": "string", "description": "Working directory (default: where Jeeves was started)"},
                "background": {
                    "type": "boolean",
                    "description": "Run it in the background (default false = this turn waits for it). Good for anything taking minutes: you get a job id immediately, then the job tool: action=output for progress, action=list for all of them, action=kill to stop one. Sandbox and approval rules are identical to the foreground."
                }
            },
            "required": ["command"]
        }),
        handler: Arc::new(move |ctx: ToolCtx, args: Value| {
            let policy = policy.clone();
            Box::pin(async move {
                let mut policy = (*policy).clone();
                policy.worktree = ctx.worktree.clone();
                let command = require_str(&args, "command")?;
                let background = args
                    .get("background")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let cwd = args
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
                    .filter(|p| p.is_dir())
                    .or_else(|| ctx.cwd.clone())
                    .or_else(|| std::env::current_dir().ok());
                if let (Some(requested), Some(workspace)) = (&cwd, &ctx.cwd) {
                    if crate::fs_write::inside_workspace(requested, workspace).is_err() {
                        return Ok(format!(
                            "Error: cwd {requested:?} is not inside this turn's working directory {workspace:?} — run_command may only run in the working directory and its subdirectories."
                        ));
                    }
                }
                let mut writable: Vec<PathBuf> = Vec::new();
                if let Some(cwd) = &cwd {
                    writable.push(cwd.clone());
                }

                let approval = ctx.approval.clone();

                if background {
                    if let Err(why) = pass_gate(&command, &policy, approval.as_deref()).await {
                        return Ok(format!("Error: {why}"));
                    }
                    let Some(jobs) = ctx.jobs.clone() else {
                        return Ok(
                            "Error: background jobs are not enabled on this machine (they are switched on together with run_command: JEEVES_EXEC=1)."
                                .to_string(),
                        );
                    };
                    let cwd_text = cwd.as_ref().map(|p| p.display().to_string());
                    return Ok(
                        match jobs
                            .spawn_guarded(
                                &ctx.session_id,
                                &command,
                                cwd_text,
                                ctx.task_run_id.clone(),
                                ctx.resource_lease.clone(),
                                ctx.worktree.clone(),
                            )
                            .await
                        {
                            Ok(id) => format!(
                            "Running in the background: {id} ({command})\nUse job(action=output) for progress, job(action=list) for all of them, job(action=kill) to stop one."
                        ),
                            Err(why) => format!("Error: {why}"),
                        },
                    );
                }

                Ok(execute_with_notice(
                    &command,
                    &policy,
                    &writable,
                    approval.as_deref(),
                    ctx.on_delta.clone(),
                    ctx.on_uncertain.clone(),
                )
                .await)
            })
        }),
    }
}
