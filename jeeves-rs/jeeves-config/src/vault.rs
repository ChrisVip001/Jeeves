//! The credential vault: secrets the model may *use* but never *see*.
//!
//! The owner stores a value under an uppercase name in Settings. The model can only write
//! `$JEEVES_SECRET_<NAME>` in a command; the Runtime injects the value into that one process's
//! environment and scrubs it from the output. Values live in the secret store (keychain first);
//! `<home>/vault.json` holds the names only, so listing never touches a secret.

use crate::models::SecretStore;
use std::path::{Path, PathBuf};

pub const ENV_PREFIX: &str = "JEEVES_SECRET_";
const MAX_NAMES: usize = 50;

fn index_path(home: &Path) -> PathBuf {
    home.join("vault.json")
}

fn account(name: &str) -> String {
    format!("vault:{name}")
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

/// The stored names, sorted. Never values.
pub fn names(home: &Path) -> Vec<String> {
    read_names(home).unwrap_or_default()
}

pub fn read_names(home: &Path) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = crate::json_file::read(&index_path(home))
        .map_err(|e| format!("could not read vault.json: {e}"))?;
    names.sort();
    Ok(names)
}

fn write_names(home: &Path, names: &[String]) -> Result<(), String> {
    crate::json_file::write(&index_path(home), names)
        .map_err(|e| format!("could not write vault.json: {e}"))
}

/// Store a value; returns where it landed (`keychain` / `file`).
pub fn put(
    store: &mut SecretStore,
    home: &Path,
    name: &str,
    value: &str,
) -> Result<&'static str, String> {
    if !valid_name(name) {
        return Err("a name is 1-40 characters of A-Z, 0-9 and _".into());
    }
    if value.is_empty() {
        return Err("the value is empty".into());
    }
    let _lock = crate::json_file::lock(home, "vault.lock").map_err(|e| e.to_string())?;
    let mut all = read_names(home)?;
    let new = !all.iter().any(|n| n == name);
    if new && all.len() >= MAX_NAMES {
        return Err(format!("the vault holds at most {MAX_NAMES} secrets"));
    }
    let landed = store.put(home, &account(name), value)?;
    if new {
        all.push(name.to_string());
        write_names(home, &all)?;
    }
    Ok(landed)
}

pub fn remove(store: &mut SecretStore, home: &Path, name: &str) -> Result<(), String> {
    let _lock = crate::json_file::lock(home, "vault.lock").map_err(|e| e.to_string())?;
    let mut all = read_names(home)?;
    if !all.iter().any(|n| n == name) {
        return Ok(());
    }
    store.remove(home, &account(name))?;
    all.retain(|n| n != name);
    write_names(home, &all)
}

pub fn get(store: &mut SecretStore, home: &Path, name: &str) -> Option<String> {
    if !names(home).iter().any(|n| n == name) {
        return None;
    }
    store.get(home, &account(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_and_the_index_holds_names_only() {
        let home = tempfile::tempdir().unwrap();
        let mut store = SecretStore::file_only();
        put(&mut store, home.path(), "GITHUB_TOKEN", "ghp_secret").unwrap();
        assert_eq!(names(home.path()), vec!["GITHUB_TOKEN"]);
        assert_eq!(
            get(&mut store, home.path(), "GITHUB_TOKEN").as_deref(),
            Some("ghp_secret")
        );
        let index = std::fs::read_to_string(home.path().join("vault.json")).unwrap();
        assert!(!index.contains("ghp_secret"));
        remove(&mut store, home.path(), "GITHUB_TOKEN").unwrap();
        assert!(names(home.path()).is_empty());
        assert!(get(&mut store, home.path(), "GITHUB_TOKEN").is_none());
    }

    #[test]
    fn bad_names_and_empty_values_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let mut store = SecretStore::file_only();
        for bad in ["", "lower", "A B", "A-B", &"X".repeat(41)] {
            assert!(put(&mut store, home.path(), bad, "v").is_err(), "{bad}");
        }
        assert!(put(&mut store, home.path(), "OK", "").is_err());
    }

    #[test]
    fn unreadable_existing_records_are_preserved_on_mutation() {
        for filename in ["vault.json", "secrets.json"] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path();
            let mut store = SecretStore::file_only();
            put(&mut store, home, "FIRST", "local-fixture").unwrap();
            let path = home.join(filename);
            std::fs::write(&path, "{incomplete").unwrap();
            assert!(
                put(&mut store, home, "SECOND", "local-fixture").is_err(),
                "{filename}"
            );
            assert!(remove(&mut store, home, "FIRST").is_err(), "{filename}");
            assert_eq!(std::fs::read_to_string(path).unwrap(), "{incomplete");
        }
    }

    #[test]
    fn simultaneous_file_backend_puts_keep_each_named_value() {
        let home = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            for n in 0..8 {
                let path = home.path();
                let gate = barrier.clone();
                scope.spawn(move || {
                    gate.wait();
                    put(
                        &mut SecretStore::file_only(),
                        path,
                        &format!("FIXTURE_{n}"),
                        "fixture",
                    )
                    .unwrap();
                });
            }
        });
        assert_eq!(read_names(home.path()).unwrap().len(), 8);
        let mut store = SecretStore::file_only();
        for n in 0..8 {
            assert_eq!(
                get(&mut store, home.path(), &format!("FIXTURE_{n}")).as_deref(),
                Some("fixture")
            );
        }
    }
}
