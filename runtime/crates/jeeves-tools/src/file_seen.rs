//! What each session last saw of a file, so a write never lands on top of a change it never
//! read.
//!
//! `read_file` and the write tools record the file's modification time, size and content digest
//! after they touch it. Before `edit`, `write_file` or `apply_patch` change an existing file,
//! `check` compares the disk with that record: when a person, an editor or a command changed the
//! file since, the write is refused and the model is told to read it again. A new timestamp on
//! the same bytes (a `touch`, a rewrite of identical text) is not a change. A file this session
//! never read or wrote has no record and stays writable, so creating or overwriting a file
//! still works in one step.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

/// Records kept before the table starts over. One per (session, file) pair; dropping them
/// all only loosens the check, it never blocks a write.
const MAX_RECORDS: usize = 4_096;

/// Files up to this size also carry a content digest. A larger file is judged by its
/// timestamp and size alone, so a check never reads more than this to compare.
const DIGEST_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    digest: Option<String>,
}

static SEEN: LazyLock<Mutex<HashMap<(String, PathBuf), Stamp>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: meta.modified().ok(),
        len: meta.len(),
        digest: digest(path, meta.len()),
    })
}

fn digest(path: &Path, len: u64) -> Option<String> {
    if len > DIGEST_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(jeeves_state::ledger::digest_of(&bytes))
}

fn key(session: &str, path: &Path) -> (String, PathBuf) {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    (session.to_string(), path)
}

/// Note the file as this session now sees it (after a read or a write).
pub fn record(session: &str, path: &Path) {
    let Some(now) = stamp(path) else {
        return;
    };
    let mut seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
    if seen.len() >= MAX_RECORDS {
        seen.clear();
    }
    seen.insert(key(session, path), now);
}

/// Forget the file for this session (it was deleted).
pub fn forget(session: &str, path: &Path) {
    let mut seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
    seen.remove(&key(session, path));
}

/// `Err` with the text for the model when the file changed on disk since this session last
/// read or wrote it. `display` is the path as the model wrote it.
pub fn check(session: &str, path: &Path, display: &str) -> Result<(), String> {
    let known = {
        let seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
        seen.get(&key(session, path)).cloned()
    };
    let Some(known) = known else {
        return Ok(());
    };
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(());
    };
    if meta.len() == known.len && meta.modified().ok() == known.modified {
        return Ok(());
    }
    let same_bytes = meta.len() == known.len
        && known.digest.is_some()
        && digest(path, meta.len()) == known.digest;
    if same_bytes {
        record(session, path);
        return Ok(());
    }
    Err(format!(
        "{display} changed on disk since you last read it (an editor, a person or a command such as a formatter wrote it). Nothing was written. read_file it again and redo the change against the current text."
    ))
}

/// The same check for a whole call, run by the registry before any approval is asked: a write
/// that will be refused must not show the user an approval card first. Covers the `path` of
/// `write_file` and `edit`, and every updated or deleted file of `apply_patch`; other tools
/// pass. The handlers check again, right before they write.
pub fn check_call(
    ctx: &crate::ToolCtx,
    name: &str,
    args: &serde_json::Value,
) -> Result<(), String> {
    let resolve = |raw: &str| crate::fs::resolve_against_cwd(ctx, crate::fs::expand_tilde(raw));
    match name {
        "write_file" | "edit" => match args.get("path").and_then(serde_json::Value::as_str) {
            Some(raw) => check(&ctx.session_id, &resolve(raw), raw),
            None => Ok(()),
        },
        "apply_patch" => {
            let Some(text) = args.get("patch").and_then(serde_json::Value::as_str) else {
                return Ok(());
            };
            let Ok(ops) = crate::patch::parse(text) else {
                return Ok(());
            };
            ops.iter().try_for_each(|op| match op {
                crate::patch::Op::Update { path, .. } | crate::patch::Op::Delete { path } => {
                    check(&ctx.session_id, &resolve(path), path)
                }
                crate::patch::Op::Add { .. } => Ok(()),
            })
        }
        _ => Ok(()),
    }
}
