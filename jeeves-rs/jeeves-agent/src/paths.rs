//! **The single containment rule of the whole repo**: `path` must land inside `root` after
//! symlink resolution.
//!
//! Two callers need exactly this rule — the write tools (may I write here?) and the task
//! verifier (may I read this acceptance condition?). Two copies of "is this path inside" would
//! drift, and the drift would be a security bug, not a cosmetic one. So the implementation lives
//! here, in the crate both sides depend on, and `jeeves_tools::fs_write` re-exports it.

use std::path::{Path, PathBuf};

/// **The single containment rule**: `path` must land inside `root` after symlink resolution.
///
/// The file itself may not exist yet (a create is not an escape), so the deepest **existing**
/// ancestor is canonicalized — every symlink that really exists in the prefix must resolve
/// inside the root. Returns `path` itself (the caller's path, now proven contained) rather than
/// the canonical ancestor: the caller wants to open/write the file it named.
pub fn inside_workspace(path: &Path, root: &Path) -> Result<PathBuf, String> {
    let root_real = root
        .canonicalize()
        .map_err(|e| format!("working directory unavailable ({}): {e}", root.display()))?;
    let real = existing_ancestor(path)
        .map_err(|e| format!("cannot resolve workspace path {}: {e}", path.display()))?;
    if real.starts_with(&root_real) {
        Ok(path.to_path_buf())
    } else {
        Err(format!(
            "{path:?} is outside the working directory ({})",
            root.display()
        ))
    }
}

/// Canonicalize the deepest existing ancestor of `path` — the file itself may not exist yet
/// (a create is not an escape), but every symlink in its real prefix must resolve inside.
fn existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
    let mut current = path;
    loop {
        match current.canonicalize() {
            Ok(real) => return Ok(real),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match current.symlink_metadata() {
                    Err(missing)
                        if missing.kind() == std::io::ErrorKind::NotFound
                            && current.file_name().is_some() => {}
                    Err(other) => return Err(other),
                    Ok(_) => return Err(e),
                }
                current = current.parent().ok_or(e)?;
            }
            Err(e) => return Err(e),
        }
    }
}
