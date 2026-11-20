//! `<home>/models.json` — multiple "vendor + model + credential" profiles, switchable at will.
//!
//! Division of labor with `settings.json`: **that one holds runtime parameters** (rounds,
//! tokens, timeouts, gates), **this one holds which vendor's model, with which key** — it
//! must be UI-editable, credentials prefer the keychain: hence a separate file and methods.
//!
//! Three things are discipline in this implementation, not optional optimizations:
//!
//! * **Credentials never default to plaintext on disk**: macOS uses the Keychain (`security`),
//!   Linux uses `secret-tool`, Windows uses the Credential Manager (`keyring` crate); only if
//!   they all fail fall back to `<home>/secrets.json` (0600 on Unix).
//! * **Tests never touch the real keychain**: `SecretStore::memory()` / `file_only()`
//!   let unit tests run in a temp dir, without system auth prompts or keychain writes.
//! * **A profile is just "config"**: whether it applies to the current session is decided
//!   by `model/use`; editing the file does not affect the turn already running.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Service name in the keychain (shared by all three platforms).
pub const SERVICE: &str = "jeeves";

mod changes;
pub use changes::{
    recover_change, remove_profile, save_profile, CredentialSlot, JOURNAL as MODEL_CHANGE_FILE,
};

/// One model profile.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelProfile {
    /// Short name chosen by the user (`ds`, `company-openai`...); used when switching.
    pub id: String,
    /// Provider id of `jeeves-provider` (one of the 12, or `custom` with base_url).
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub model: Option<String>,
    /// Cheap model: for the retrieval gate, consolidation, and the goal judge; provider default if absent.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub small_model: Option<String>,
    /// Self-hosted/proxied endpoint. Empty = the provider's own.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub base_url: Option<String>,
    /// Where credentials come from: `env:VAR` / `keychain` / `file`; `None` = this vendor
    /// needs no key (ollama, LM Studio). **Only a pointer is stored here, never the key itself.**
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub key_source: Option<String>,
    #[serde(default, skip_serializing_if = "CredentialSlot::is_legacy")]
    pub credential_slot: CredentialSlot,
    /// Whether **this profile's model** accepts images. Absent = the vendor's default
    /// (catalog row); `false` = text-only even on a vendor that ships vision models.
    /// Declared when the model is added, because "which model" and "what it can see" are
    /// the same decision — one vendor can ship both kinds.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub vision: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub capabilities: Option<jeeves_protocol::ModelCapabilities>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelsFile {
    /// id of the currently active profile.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub active: Option<String>,
    #[serde(default)]
    pub profiles: Vec<ModelProfile>,
}

pub fn models_path(home: &Path) -> PathBuf {
    home.join("models.json")
}

fn secrets_path(home: &Path) -> PathBuf {
    home.join("secrets.json")
}

pub fn load_models(home: &Path) -> ModelsFile {
    read_models(home).unwrap_or_default()
}

pub fn read_models(home: &Path) -> std::io::Result<ModelsFile> {
    crate::json_file::read(&models_path(home))
}

pub fn save_models(home: &Path, models: &ModelsFile) -> std::io::Result<()> {
    crate::json_file::write(&models_path(home), models)
}

pub fn find<'a>(models: &'a ModelsFile, id: &str) -> Option<&'a ModelProfile> {
    models.profiles.iter().find(|p| p.id == id)
}

pub fn active_profile(models: &ModelsFile) -> Option<&ModelProfile> {
    models.active.as_deref().and_then(|id| find(models, id))
}

/// Write (or overwrite by same id) one profile. **No key given keeps the original key_source** —
/// "switching" or "changing models" must not force the user to re-enter the key.
pub fn upsert(models: &mut ModelsFile, incoming: ModelProfile) {
    match models.profiles.iter_mut().find(|p| p.id == incoming.id) {
        Some(existing) => {
            let keep_key = incoming
                .key_source
                .clone()
                .or_else(|| existing.key_source.clone());
            *existing = ModelProfile {
                credential_slot: if incoming.key_source.is_none() {
                    existing.credential_slot
                } else {
                    incoming.credential_slot
                },
                key_source: keep_key,
                ..incoming
            };
        }
        None => models.profiles.push(incoming),
    }
}

pub fn remove(models: &mut ModelsFile, id: &str) -> bool {
    let before = models.profiles.len();
    models.profiles.retain(|p| p.id != id);
    if models.active.as_deref() == Some(id) {
        models.active = None;
    }
    models.profiles.len() != before
}

/// Apply the active profile's **config** into Settings (credentials fetched separately, see
/// `apply_active`). Leaves `settings.json` alone: that file holds runtime parameters.
pub fn apply_active(
    models: &ModelsFile,
    home: &Path,
    store: &mut dyn SecretVault,
    settings: &mut crate::Settings,
) {
    let Some(profile) = active_profile(models) else {
        return;
    };
    apply_profile(profile, home, store, settings);
}

/// Apply **any** profile into Settings (used when routing picks models by tier — shares
/// the same key-fetching path as apply_active).
pub fn apply_profile(
    profile: &ModelProfile,
    home: &Path,
    store: &mut dyn SecretVault,
    settings: &mut crate::Settings,
) {
    settings.provider = profile.provider.clone();
    settings.model = profile.model.clone();
    settings.small_model = profile.small_model.clone();
    settings.base_url = profile.base_url.clone();
    settings.api_key = resolve_key(profile, home, store);
    settings.vision = profile.vision;
    settings.model_capabilities = profile.capabilities.clone();
}

/// Fetch the real key per `key_source`. `env:` reads the env var directly (**never copied
/// into files**); `keychain`/`file` go through `SecretStore`; absent = None (local models).
pub fn resolve_key(
    profile: &ModelProfile,
    home: &Path,
    store: &mut dyn SecretVault,
) -> Option<String> {
    let source = profile.key_source.as_deref()?;
    match source.strip_prefix("env:") {
        Some(var) => std::env::var(var).ok().filter(|v| !v.trim().is_empty()),
        None if profile.credential_slot.is_legacy() => store.get(home, &profile.id),
        None => store
            .versioned_credential(home, &profile.credential_slot.account(&profile.id))
            .map(|(value, _)| value),
    }
}

/// Where credentials live. `Auto` is the default: use the system keychain when possible, else file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretBackend {
    Auto,
    File,
    /// Lives only in process memory: for tests, and for users explicitly choosing "no disk".
    Memory,
}

pub struct SecretStore {
    backend: SecretBackend,
    memory: HashMap<String, String>,
}

impl SecretStore {
    pub fn auto() -> Self {
        Self {
            backend: SecretBackend::Auto,
            memory: HashMap::new(),
        }
    }

    pub fn file_only() -> Self {
        Self {
            backend: SecretBackend::File,
            memory: HashMap::new(),
        }
    }

    pub fn memory() -> Self {
        Self {
            backend: SecretBackend::Memory,
            memory: HashMap::new(),
        }
    }

    pub fn backend(&self) -> SecretBackend {
        self.backend
    }

    /// Switch backend. **Only for tests and "never touch the keychain" scenarios**: the default
    /// is `Auto`; unit tests must pin `File` (temp dir) or `Memory` — no system auth prompts in CI.
    pub fn set_backend(&mut self, backend: SecretBackend) {
        self.backend = backend;
    }

    /// Store a key, returning **where it actually landed** (`keychain` / `file` / `memory`) —
    /// UI and logs must report the truth: never imply keychain when it is a plaintext file.
    pub fn put(
        &mut self,
        home: &Path,
        account: &str,
        secret: &str,
    ) -> Result<&'static str, String> {
        self.put_guarded(home, account, secret, None)
    }

    fn put_guarded(
        &mut self,
        home: &Path,
        account: &str,
        secret: &str,
        guard: Option<&Arc<File>>,
    ) -> Result<&'static str, String> {
        match self.backend {
            SecretBackend::Memory => {
                self.memory.insert(account.to_string(), secret.to_string());
                Ok("memory")
            }
            SecretBackend::File => write_file_secret(home, account, secret).map(|()| "file"),
            SecretBackend::Auto => {
                if os_keychain_put(&scoped(home, account), secret, guard) {
                    Ok("keychain")
                } else {
                    write_file_secret(home, account, secret).map(|()| "file")
                }
            }
        }
    }

    pub fn get(&mut self, home: &Path, account: &str) -> Option<String> {
        self.get_with_source(home, account).map(|(value, _)| value)
    }

    /// Resolve the actual storage backend along with the value.
    pub fn get_with_source(
        &mut self,
        home: &Path,
        account: &str,
    ) -> Option<(String, &'static str)> {
        self.lookup(home, account, true)
    }

    fn lookup(
        &mut self,
        home: &Path,
        account: &str,
        allow_legacy: bool,
    ) -> Option<(String, &'static str)> {
        match self.backend {
            SecretBackend::Memory => self.memory.get(account).cloned().map(|v| (v, "memory")),
            SecretBackend::File => read_file_secret(home, account).map(|v| (v, "file")),
            SecretBackend::Auto => {
                let key = scoped(home, account);
                os_keychain_get(&key)
                    .or_else(|| {
                        if !allow_legacy {
                            return None;
                        }
                        let legacy = os_keychain_get(account)?;
                        os_keychain_put(&key, &legacy, None);
                        Some(legacy)
                    })
                    .map(|v| (v, "keychain"))
                    .or_else(|| read_file_secret(home, account).map(|v| (v, "file")))
            }
        }
    }

    /// Pin a file-only write in its recovery record when no native helper was installed.
    /// Recovery must also work if the process dies before the first credential write.
    fn model_writes_to_file(&self) -> bool {
        if self.backend != SecretBackend::Auto {
            return self.backend == SecretBackend::File;
        }
        let helper = if cfg!(target_os = "macos") {
            "security"
        } else if cfg!(target_os = "linux") {
            "secret-tool"
        } else {
            return false;
        };
        std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path).all(|dir| {
                matches!(std::fs::metadata(dir.join(helper)), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
            })
        })
    }

    fn remove_model_credential(
        &mut self,
        home: &Path,
        account: &str,
        slot: CredentialSlot,
        source: Option<&str>,
        guard: &Arc<File>,
    ) -> Result<(), String> {
        if self.backend != SecretBackend::Auto {
            return self.remove(home, account);
        }
        if slot.is_legacy() {
            return self.remove_auto(home, account, source == Some("file"), Some(guard));
        }
        match source {
            Some("memory") => {
                self.memory.remove(account);
                Ok(())
            }
            Some("keychain") => os_keychain_remove(&scoped(home, account), false, Some(guard)),
            Some("file") => {
                os_keychain_remove(&scoped(home, account), true, Some(guard))?;
                remove_file_secret(home, account)
            }
            _ => {
                os_keychain_remove(
                    &scoped(home, account),
                    read_file_secret(home, account).is_some(),
                    Some(guard),
                )?;
                remove_file_secret(home, account)
            }
        }
    }

    fn remove_auto(
        &mut self,
        home: &Path,
        account: &str,
        known_file: bool,
        guard: Option<&Arc<File>>,
    ) -> Result<(), String> {
        let file_fallback = known_file || read_file_secret(home, account).is_some();
        os_keychain_remove(&scoped(home, account), file_fallback, guard)?;
        os_keychain_remove(account, file_fallback, guard)?;
        remove_file_secret(home, account)
    }

    pub fn remove(&mut self, home: &Path, account: &str) -> Result<(), String> {
        match self.backend {
            SecretBackend::Memory => {
                self.memory.remove(account);
            }
            SecretBackend::File => {
                remove_file_secret(home, account)?;
            }
            SecretBackend::Auto => {
                self.remove_auto(home, account, false, None)?;
            }
        }
        Ok(())
    }
}

/// The one place secrets pass through: LLM keys, connector tokens and MCP tokens all come
/// through this seam, sooner or later.
///
/// Today's implementation is [`SecretStore`] (keychain / 0600 file / memory). The trait stays
/// for the **second backend**: when a server-shaped KMS envelope appears, only an
/// implementation is added and callers like `apply_profile` do not change a line. The
/// connector and MCP token files have not moved in yet — their rotation and locking rules
/// (0600 plus temp-file rename plus a cross-process lock) deserve a round of their own; the
/// time to move them is when the product actually ships a server edition.
pub trait SecretVault {
    /// Store a secret and return where it **actually landed** (`keychain` / `file` /
    /// `memory`) — the UI and the logs have to tell the truth: never imply "keychain" when
    /// it is really a plaintext file.
    fn put(&mut self, home: &Path, account: &str, secret: &str) -> Result<&'static str, String>;

    /// Read it back. `None` = never stored (a profile or service that needs no key at all).
    fn get(&mut self, home: &Path, account: &str) -> Option<String>;

    /// Return the actual backend of a stored credential, never its value.
    fn location(&mut self, home: &Path, account: &str) -> Option<&'static str>;

    /// Read an exclusively owned model slot without importing historical unscoped entries.
    fn versioned_credential(
        &mut self,
        home: &Path,
        account: &str,
    ) -> Option<(String, &'static str)> {
        self.get(home, account).zip(self.location(home, account))
    }

    /// Delete it (the keychain entry and the file fallback are both cleared).
    fn remove(&mut self, home: &Path, account: &str) -> Result<(), String>;
}

impl SecretVault for SecretStore {
    fn put(&mut self, home: &Path, account: &str, secret: &str) -> Result<&'static str, String> {
        SecretStore::put(self, home, account, secret)
    }

    fn get(&mut self, home: &Path, account: &str) -> Option<String> {
        SecretStore::get(self, home, account)
    }

    fn location(&mut self, home: &Path, account: &str) -> Option<&'static str> {
        self.get_with_source(home, account)
            .map(|(_, source)| source)
    }

    fn versioned_credential(
        &mut self,
        home: &Path,
        account: &str,
    ) -> Option<(String, &'static str)> {
        self.lookup(home, account, false)
    }

    fn remove(&mut self, home: &Path, account: &str) -> Result<(), String> {
        SecretStore::remove(self, home, account)
    }
}

fn read_secrets(home: &Path) -> Result<HashMap<String, String>, String> {
    crate::json_file::read(&secrets_path(home))
        .map_err(|e| format!("could not read secrets.json: {e}"))
}

fn write_file_secret(home: &Path, account: &str, secret: &str) -> Result<(), String> {
    let _lock = crate::json_file::lock(home, "secrets.lock").map_err(|e| e.to_string())?;
    let mut all = read_secrets(home)?;
    all.insert(account.to_owned(), secret.to_owned());
    crate::json_file::write(&secrets_path(home), &all)
        .map_err(|e| format!("could not save secrets.json: {e}"))
}

fn read_file_secret(home: &Path, account: &str) -> Option<String> {
    read_secrets(home).ok()?.remove(account)
}

fn remove_file_secret(home: &Path, account: &str) -> Result<(), String> {
    let _lock = crate::json_file::lock(home, "secrets.lock").map_err(|e| e.to_string())?;
    let mut all = read_secrets(home)?;
    if all.remove(account).is_some() {
        crate::json_file::write(&secrets_path(home), &all)
            .map_err(|e| format!("could not save secrets.json: {e}"))?;
    }
    Ok(())
}

/// Run a keychain command, **with a timeout**.
///
/// `security` / `secret-tool` may pop dialogs or hang in unattended environments (CI,
/// remote sessions, ssh without GUI). Freezing the app-server on one credential operation
/// is unacceptable. A timeout lets the caller apply its read or storage fallback.
fn run_limited(
    command: &mut std::process::Command,
    stdin_secret: Option<&str>,
    guard: Option<&Arc<File>>,
) -> Option<String> {
    let out = keychain_output(command, stdin_secret, guard).ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn keychain_output(
    command: &mut std::process::Command,
    stdin_secret: Option<&str>,
    guard: Option<&Arc<File>>,
) -> std::io::Result<std::process::Output> {
    use std::io::{self, Write};
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    #[cfg(unix)]
    if let Some(guard) = guard {
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let guard = guard.clone();
        unsafe {
            command.pre_exec(move || {
                if libc::fcntl(guard.as_raw_fd(), libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = guard;
    let mut child = command
        .stdin(if stdin_secret.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(KEYCHAIN_TIMEOUT_SECS);
    std::thread::scope(|scope| {
        let writer = stdin_secret
            .zip(child.stdin.take())
            .map(|(secret, mut stdin)| scope.spawn(move || stdin.write_all(secret.as_bytes())));
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20))
                }
                outcome => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(outcome
                        .err()
                        .unwrap_or_else(|| io::Error::from(io::ErrorKind::TimedOut)));
                }
            }
        }
        if let Some(writer) = writer {
            writer
                .join()
                .map_err(|_| io::Error::other("keychain input failed"))??;
        }
        child.wait_with_output()
    })
}

/// The keychain account for `account` in this home. The keychain is shared by every home on
/// the machine, and a home is a tenant: without the suffix two homes with a profile of the
/// same name would read and overwrite each other's key.
fn scoped(home: &Path, account: &str) -> String {
    use sha2::{Digest, Sha256};
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let digest = Sha256::digest(home.to_string_lossy().as_bytes());
    let tag: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    format!("{account}@{tag}")
}

/// Wait cap for keychain commands.
const KEYCHAIN_TIMEOUT_SECS: u64 = 5;

/// The single entry point to the platform keychain. macOS: `security add-generic-password -U -s jeeves -a <account>
/// -w <key>`；Linux: `secret-tool store` reads from stdin (**never argv** — argv is visible
/// to all)；Windows: Credential Manager（keyring crate，CredWrite/CredRead）。
fn os_keychain_put(account: &str, secret: &str, guard: Option<&Arc<File>>) -> bool {
    if cfg!(target_os = "macos") {
        return run_limited(
            std::process::Command::new("security").args([
                "add-generic-password",
                "-U",
                "-s",
                SERVICE,
                "-a",
                account,
                "-w",
                secret,
            ]),
            None,
            guard,
        )
        .is_some();
    }
    if cfg!(target_os = "linux") {
        return run_limited(
            std::process::Command::new("secret-tool").args([
                "store",
                "--label",
                &format!("{SERVICE} {account}"),
                "service",
                SERVICE,
                "account",
                account,
            ]),
            Some(secret),
            guard,
        )
        .is_some();
    }
    #[cfg(target_os = "windows")]
    {
        if win::put(account, secret, guard.cloned()) {
            return true;
        }
    }
    false
}

fn os_keychain_get(account: &str) -> Option<String> {
    if cfg!(target_os = "macos") {
        return run_limited(
            std::process::Command::new("security").args([
                "find-generic-password",
                "-s",
                SERVICE,
                "-a",
                account,
                "-w",
            ]),
            None,
            None,
        )
        .map(|out| out.trim().to_string())
        .filter(|s| !s.is_empty());
    }
    if cfg!(target_os = "linux") {
        return run_limited(
            std::process::Command::new("secret-tool")
                .args(["lookup", "service", SERVICE, "account", account]),
            None,
            None,
        )
        .map(|out| out.trim().to_string())
        .filter(|s| !s.is_empty());
    }
    #[cfg(target_os = "windows")]
    {
        return win::get(account);
    }
    #[cfg(not(target_os = "windows"))]
    None
}

fn delete_with(
    command: &mut std::process::Command,
    macos: bool,
    file_fallback: bool,
    guard: Option<&Arc<File>>,
) -> Result<(), String> {
    let out = match keychain_output(command, None, guard) {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && file_fallback => return Ok(()),
        Err(e) => {
            return Err(format!(
                "system credential deletion unavailable: {}",
                e.kind()
            ))
        }
    };
    let absent = if macos {
        out.status.code() == Some(44)
    } else {
        out.status.code() == Some(1) && out.stderr.is_empty()
    };
    if out.status.success() || absent {
        Ok(())
    } else {
        Err(format!(
            "system credential deletion failed ({})",
            out.status
        ))
    }
}

fn os_keychain_remove(
    account: &str,
    file_fallback: bool,
    guard: Option<&Arc<File>>,
) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        return delete_with(
            std::process::Command::new("security").args([
                "delete-generic-password",
                "-s",
                SERVICE,
                "-a",
                account,
            ]),
            true,
            file_fallback,
            guard,
        );
    }
    if cfg!(target_os = "linux") {
        return delete_with(
            std::process::Command::new("secret-tool")
                .args(["clear", "service", SERVICE, "account", account]),
            false,
            file_fallback,
            guard,
        );
    }
    #[cfg(target_os = "windows")]
    if !win::remove(account, guard.cloned()) {
        return Err("system credential deletion failed".into());
    }
    Ok(())
}

/// Compiled on Windows only. The command-line paths on macOS and Linux have shipped, so they
/// are not being replaced; Windows previously had the command-line embarrassment of
/// "writes fine, cannot read back" (see limitations), and `keyring` goes through
/// CredWrite/CredRead, which works both ways.
#[cfg(target_os = "windows")]
mod win {
    use super::{Arc, File};
    use keyring::Entry;

    /// A keyring call blocks the OS thread, so it is wrapped in a 5-second timeout — the same
    /// discipline as `security`/`secret-tool`: a credentials call freezing the app-server is
    /// not acceptable. Timeout = failure, and it falls back to the file.
    fn limited<T>(
        account: &str,
        guard: Option<Arc<File>>,
        op: impl FnOnce(&Entry) -> Result<T, keyring::Error> + Send + 'static,
    ) -> Option<T>
    where
        T: Send + 'static,
    {
        let entry = Entry::new(super::SERVICE, account).ok()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = op(&entry).ok();
            drop(guard);
            let _ = tx.send(result);
        });
        rx.recv_timeout(std::time::Duration::from_secs(super::KEYCHAIN_TIMEOUT_SECS))
            .ok()
            .flatten()
    }

    pub fn put(account: &str, secret: &str, guard: Option<Arc<File>>) -> bool {
        let secret = secret.to_string();
        limited(account, guard, move |entry| entry.set_password(&secret)).is_some()
    }

    pub fn get(account: &str) -> Option<String> {
        limited(account, None, |entry| entry.get_password())
    }

    pub fn remove(account: &str, guard: Option<Arc<File>>) -> bool {
        limited(account, guard, |entry| match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e),
        })
        .is_some()
    }
}

#[cfg(test)]
mod models_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn keychain_delete_distinguishes_missing_items_from_a_backend_failure() {
        use std::process::Command;
        for (script, macos, expected) in [
            ("exit 0", true, true),
            ("exit 44", true, true),
            ("exit 1", false, true),
            ("exit 36", true, false),
            ("echo fixture-error >&2; exit 1", false, false),
        ] {
            let result = delete_with(
                Command::new("/bin/sh").args(["-c", script]),
                macos,
                false,
                None,
            );
            assert_eq!(result.is_ok(), expected);
            if let Err(message) = result {
                assert!(!message.contains("fixture-error"));
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing-helper");
        assert!(delete_with(&mut Command::new(&missing), false, true, None).is_ok());
        assert!(delete_with(&mut Command::new(&missing), false, false, None).is_err());
    }

    fn profile(id: &str, provider: &str) -> ModelProfile {
        ModelProfile {
            id: id.to_string(),
            provider: provider.to_string(),
            model: None,
            small_model: None,
            base_url: None,
            key_source: None,
            credential_slot: CredentialSlot::Legacy,
            vision: None,
            capabilities: None,
        }
    }

    /// The Vault trait is the only seam: a full store/read/delete round works through
    /// `&mut dyn SecretVault` too — a future server KMS backend plugs in here, and callers
    /// (apply_profile and friends) change nothing.
    #[test]
    fn the_vault_trait_is_the_only_seam() {
        let home = Path::new("/tmp/jeeves-vault-trait-test");
        let mut vault: Box<dyn SecretVault> = Box::new(SecretStore::memory());
        assert_eq!(vault.get(home, "llm:ds"), None);
        assert_eq!(
            vault.put(home, "llm:ds", "sk-test").expect("store"),
            "memory"
        );
        assert_eq!(vault.get(home, "llm:ds").as_deref(), Some("sk-test"));
        vault.remove(home, "llm:ds").unwrap();
        assert_eq!(vault.get(home, "llm:ds"), None);
    }

    /// Memory backend: **never touches the system keychain or disk** — CI must never see auth prompts.
    #[test]
    fn the_memory_backend_never_touches_the_disk() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = SecretStore::memory();
        assert_eq!(store.put(dir.path(), "ds", "sk-1").unwrap(), "memory");
        assert_eq!(store.get(dir.path(), "ds").as_deref(), Some("sk-1"));
        assert!(
            !secrets_path(dir.path()).exists(),
            "the memory backend must not write secrets.json"
        );
    }

    /// File fallback: stores and reads back, with (Unix) permissions tightened to 0600.
    #[test]
    fn the_file_backend_stores_and_reads_back() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = SecretStore::file_only();
        assert_eq!(store.put(dir.path(), "ds", "sk-2").unwrap(), "file");
        assert_eq!(store.get(dir.path(), "ds").as_deref(), Some("sk-2"));
        store.remove(dir.path(), "ds").unwrap();
        assert_eq!(store.get(dir.path(), "ds"), None);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut store = SecretStore::file_only();
            store.put(dir.path(), "ds", "sk-3").unwrap();
            let mode = std::fs::metadata(secrets_path(dir.path()))
                .expect("the file is there")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "the plaintext fallback should be 0600, actually {mode:o}"
            );
        }
    }

    #[test]
    fn failed_publication_preserves_a_legacy_key_and_disposes_of_the_staged_version() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut store = SecretStore::file_only();
        let mut file = ModelsFile::default();
        let mut old = profile("old", "openai");
        old.key_source = Some(store.put(home, &old.id, "fixture-first").unwrap().into());
        upsert(&mut file, old.clone());
        save_models(home, &file).unwrap();

        let path = models_path(home);
        let backup = home.join("fixture-original");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        for id in ["old", "new"] {
            let attempted = save_profile(
                home,
                &mut file,
                &mut store,
                profile(id, "openai"),
                Some("fixture-second"),
                true,
            );
            assert!(attempted.unwrap_err().contains("write models.json"));
            assert_eq!(
                read_secrets(home).unwrap(),
                HashMap::from([("old".into(), "fixture-first".into())])
            );
            assert!(!home.join(MODEL_CHANGE_FILE).exists());
        }
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(backup, path).unwrap();
        let mut live = read_models(home).unwrap();
        assert!(recover_change(home, &mut live, &mut store)
            .unwrap()
            .is_none());
        assert_eq!(
            resolve_key(&live.profiles[0], home, &mut store).as_deref(),
            Some("fixture-first")
        );

        save_profile(
            home,
            &mut live,
            &mut store,
            old,
            Some("fixture-second"),
            true,
        )
        .unwrap();
        assert_eq!(
            resolve_key(&read_models(home).unwrap().profiles[0], home, &mut store).as_deref(),
            Some("fixture-second")
        );
        assert_eq!(read_secrets(home).unwrap().len(), 1);
        assert!(store.get(home, "old").is_none());
        let mut env_profile = live.profiles[0].clone();
        env_profile.key_source = Some("env:JEEVES_TEST_ONLY_MODEL_KEY".into());
        env_profile.credential_slot = Default::default();
        save_profile(home, &mut live, &mut store, env_profile, None, true).unwrap();
        assert!(read_secrets(home).unwrap().is_empty());
        assert_eq!(
            read_models(home).unwrap().profiles[0].key_source.as_deref(),
            Some("env:JEEVES_TEST_ONLY_MODEL_KEY")
        );
    }

    #[test]
    fn interrupted_profile_deletion_finishes_after_metadata_becomes_writable() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut store = SecretStore::file_only();
        let mut file = ModelsFile::default();
        save_profile(
            home,
            &mut file,
            &mut store,
            profile("p", "openai"),
            Some("fixture-only"),
            true,
        )
        .unwrap();
        let path = models_path(home);
        let backup = home.join("fixture-original");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(remove_profile(home, &mut file, &mut store, "p").is_err());
        assert!(file.profiles.is_empty());
        assert!(file.active.is_none());
        assert!(read_secrets(home).unwrap().is_empty());
        assert!(home.join(MODEL_CHANGE_FILE).is_file());
        std::fs::remove_dir(&path).unwrap();
        std::fs::rename(backup, path).unwrap();
        let mut live = read_models(home).unwrap();
        assert_eq!(
            recover_change(home, &mut live, &mut store)
                .unwrap()
                .as_deref(),
            Some("p")
        );
        assert!(read_models(home).unwrap().profiles.is_empty());
        assert!(recover_change(home, &mut live, &mut store)
            .unwrap()
            .is_none());
    }

    /// Overwriting a same-id profile **keeps its key_source** — no forced key re-entry.
    #[test]
    fn upserting_keeps_the_existing_key_pointer() {
        let mut models = ModelsFile::default();
        upsert(
            &mut models,
            ModelProfile {
                key_source: Some("keychain".to_string()),
                ..profile("ds", "deepseek")
            },
        );
        upsert(
            &mut models,
            ModelProfile {
                model: Some("deepseek-reasoner".to_string()),
                ..profile("ds", "deepseek")
            },
        );
        assert_eq!(
            models.profiles.len(),
            1,
            "the same id should overwrite, not add a second entry"
        );
        assert_eq!(
            models.profiles[0].model.as_deref(),
            Some("deepseek-reasoner")
        );
        assert_eq!(
            models.profiles[0].key_source.as_deref(),
            Some("keychain"),
            "not passing a key must not clear the existing credential pointer"
        );
    }

    /// Switching and removing: active stays consistent; removing it leaves no dangling pointer.
    #[test]
    fn switching_and_removing_keep_active_consistent() {
        let mut models = ModelsFile::default();
        upsert(&mut models, profile("ds", "deepseek"));
        upsert(&mut models, profile("oa", "openai"));
        models.active = Some("ds".to_string());
        assert_eq!(
            active_profile(&models).map(|p| p.provider.as_str()),
            Some("deepseek")
        );

        models.active = Some("oa".to_string());
        assert_eq!(active_profile(&models).map(|p| p.id.as_str()), Some("oa"));

        assert!(remove(&mut models, "oa"));
        assert!(
            models.active.is_none(),
            "deleting the active profile must not leave an `active` that points at nothing"
        );
        assert!(active_profile(&models).is_none());
        assert!(!remove(&mut models, "not-there"));
    }

    /// Applying into Settings: provider/model/base_url in place, key from the credential backend.
    #[test]
    fn applying_the_active_profile_fills_settings_including_the_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = SecretStore::memory();
        store.put(dir.path(), "ds", "sk-from-store").unwrap();

        let mut models = ModelsFile::default();
        upsert(
            &mut models,
            ModelProfile {
                model: Some("deepseek-flash".to_string()),
                small_model: Some("deepseek-flash".to_string()),
                base_url: Some("https://api.deepseek.com/v1".to_string()),
                key_source: Some("keychain".to_string()),
                ..profile("ds", "deepseek")
            },
        );
        models.active = Some("ds".to_string());

        let mut settings = crate::Settings::default();
        apply_active(&models, dir.path(), &mut store, &mut settings);
        assert_eq!(settings.provider, "deepseek");
        assert_eq!(settings.model.as_deref(), Some("deepseek-flash"));
        assert_eq!(
            settings.base_url.as_deref(),
            Some("https://api.deepseek.com/v1")
        );
        assert_eq!(settings.api_key.as_deref(), Some("sk-from-store"));
    }

    /// An `env:` source reads the environment directly, **never copied into any file**.
    #[test]
    fn an_env_key_source_is_read_from_the_environment() {
        let dir = tempfile::tempdir().expect("temp dir");
        let var = "JEEVES_TEST_MODELS_KEY";
        std::env::set_var(var, "sk-from-env");
        let mut models = ModelsFile::default();
        upsert(
            &mut models,
            ModelProfile {
                key_source: Some(format!("env:{var}")),
                ..profile("env", "deepseek")
            },
        );
        models.active = Some("env".to_string());
        let mut settings = crate::Settings::default();
        apply_active(
            &models,
            dir.path(),
            &mut SecretStore::memory(),
            &mut settings,
        );
        assert_eq!(settings.api_key.as_deref(), Some("sk-from-env"));
        assert!(
            !secrets_path(dir.path()).exists(),
            "the env source must not copy the key into a file"
        );
        std::env::remove_var(var);
    }

    /// No active profile leaves Settings alone — no config in the file ≠ clearing the config.
    #[test]
    fn no_active_profile_leaves_settings_alone() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut settings = crate::Settings {
            provider: "ollama".to_string(),
            ..crate::Settings::default()
        };
        apply_active(
            &ModelsFile::default(),
            dir.path(),
            &mut SecretStore::memory(),
            &mut settings,
        );
        assert_eq!(settings.provider, "ollama");
    }

    /// `vision` follows the config: unset = whatever the vendor defaults to, set = this
    /// profile decides. One vendor ships both vision and text-only models, so this is the
    /// only place that can say which is which.
    #[test]
    fn applying_a_profile_carries_its_vision_declaration() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut store = SecretStore::memory();
        let mut settings = crate::Settings::default();
        assert_eq!(settings.vision, None, "follows the vendor by default");

        let mut models = ModelsFile::default();
        upsert(
            &mut models,
            ModelProfile {
                vision: Some(true),
                ..profile("ds", "deepseek")
            },
        );
        models.active = Some("ds".to_string());
        apply_active(&models, dir.path(), &mut store, &mut settings);
        assert_eq!(
            settings.vision,
            Some(true),
            "the declaration must reach the runtime settings"
        );

        upsert(
            &mut models,
            ModelProfile {
                vision: Some(false),
                ..profile("ds", "deepseek")
            },
        );
        apply_active(&models, dir.path(), &mut store, &mut settings);
        assert_eq!(
            settings.vision,
            Some(false),
            "switching to a text-only model must take the capability back"
        );
    }

    /// Read/write round trip: the file holds only config and credential **pointers**, never keys.
    #[test]
    fn the_file_round_trips_and_never_contains_the_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut models = ModelsFile::default();
        upsert(
            &mut models,
            ModelProfile {
                key_source: Some("keychain".to_string()),
                ..profile("ds", "deepseek")
            },
        );
        models.active = Some("ds".to_string());
        save_models(dir.path(), &models).expect("write");

        let raw = std::fs::read_to_string(models_path(dir.path())).expect("read");
        assert!(
            !raw.contains("sk-"),
            "no key may appear in models.json: {raw}"
        );

        let back = load_models(dir.path());
        assert_eq!(back.active.as_deref(), Some("ds"));
        assert_eq!(back.profiles.len(), 1);
        assert_eq!(back.profiles[0].provider, "deepseek");
    }
}

#[cfg(test)]
mod scope_tests {
    use super::scoped;

    #[test]
    fn two_homes_never_share_a_keychain_account() {
        let a = tempfile::tempdir().expect("temporary directory");
        let b = tempfile::tempdir().expect("temporary directory");
        assert_ne!(scoped(a.path(), "ds"), scoped(b.path(), "ds"));
        assert_eq!(
            scoped(a.path(), "ds"),
            scoped(a.path(), "ds"),
            "stable for one home"
        );
        assert!(scoped(a.path(), "ds").starts_with("ds@"));
    }
}
