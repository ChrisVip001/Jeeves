//! `jeeves home export <dir>` / `jeeves home import <dir>` — a home is a tenant, and this moves one.
//!
//! The same operation serves a backup, moving between machines, local ⇄ cloud, and a control
//! plane provisioning a Pod. The bundle is a plain directory (archive it with `tar` if you need
//! one file): `state.db` as a consistent snapshot, every other file copied as it is, and
//! `jeeves-home.json` saying which format and schema it holds.
//!
//! Left out on purpose: lock files, the live database files, `secrets.json` (a plaintext key
//! fallback) and OAuth tokens (`mcp-auth/`, `connector-auth/`). Keys in the system keychain
//! never lived in the home. After an import, keys and sign-ins are given again.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::json;

const MANIFEST: &str = "jeeves-home.json";
const FORMAT: i64 = 1;
const SKIP: &[&str] = &[
    "state.db",
    "state.db-wal",
    "state.db-shm",
    "secrets.json",
    jeeves_config::models::MODEL_CHANGE_FILE,
    "mcp-auth",
    "connector-auth",
    MANIFEST,
];

pub async fn run(home: &Path, args: &[String]) -> Result<()> {
    match args {
        [action, dir] if action == "export" => export(home, Path::new(dir)).await,
        [action, dir] if action == "import" => import(home, Path::new(dir)).await,
        _ => bail!("Usage: jeeves home export <dir> | jeeves home import <dir>"),
    }
}

async fn export(home: &Path, out: &Path) -> Result<()> {
    let destination = prepare_destination(home, out)?;
    let stage = tempfile::tempdir_in(
        destination
            .parent()
            .context("export destination has no parent")?,
    )?;
    let out = stage.path();
    let pool = jeeves_state::open(&home.join("state.db")).await?;
    let schema = jeeves_state::db::snapshot(&pool, &out.join("state.db")).await?;
    pool.close().await;
    let files = copy_tree(home, out)?;
    let manifest = json!({
        "format": FORMAT,
        "schema": schema,
        "jeeves": env!("CARGO_PKG_VERSION"),
        "exportedAt": chrono::Utc::now().to_rfc3339(),
        "left out": SKIP.iter().filter(|name| **name != MANIFEST).collect::<Vec<_>>(),
    });
    std::fs::write(out.join(MANIFEST), serde_json::to_string_pretty(&manifest)?)?;
    publish(stage, &destination)?;
    println!(
        "Exported {} to {} (schema {schema}, {files} files). Keys and sign-ins are not included.",
        home.display(),
        destination.display()
    );
    Ok(())
}

async fn import(home: &Path, bundle: &Path) -> Result<()> {
    let raw = std::fs::read_to_string(bundle.join(MANIFEST)).with_context(|| {
        format!(
            "{} has no {MANIFEST}; not a Jeeves home export",
            bundle.display()
        )
    })?;
    let manifest: serde_json::Value = serde_json::from_str(&raw)?;
    if manifest["format"].as_i64() != Some(FORMAT) {
        bail!(
            "unknown export format {}; this jeeves reads format {FORMAT}",
            manifest["format"]
        );
    }
    let schema = manifest["schema"]
        .as_i64()
        .filter(|v| *v >= 0)
        .context("export manifest requires a nonnegative integer schema")?;
    let known = jeeves_state::db::known_schema_version();
    if schema > known && !matches!(schema, 24 | 25) {
        bail!("the export has schema {schema}, newer than this jeeves knows ({known}); upgrade jeeves first");
    }
    let db = bundle.join("state.db");
    if !std::fs::symlink_metadata(&db)
        .context("export has no state.db")?
        .is_file()
    {
        bail!("export state.db is not a regular file");
    }
    let destination = prepare_destination(bundle, home)?;
    let stage = tempfile::tempdir_in(
        destination
            .parent()
            .context("import destination has no parent")?,
    )?;
    let files = copy_tree(bundle, stage.path())?;
    let staged_db = stage.path().join("state.db");
    std::fs::copy(db, &staged_db)?;
    let actual = jeeves_state::db::snapshot_version(&staged_db)
        .await
        .context("could not verify the exported database schema")?;
    if schema != actual {
        bail!("manifest schema {schema} differs from database schema {actual}");
    }
    let pool = jeeves_state::open(&staged_db).await?;
    let now = jeeves_state::db::schema_version(&pool).await?;
    pool.close().await;
    publish(stage, &destination)?;
    println!(
        "Imported into {} (schema {schema} → {now}, {} files). Add model keys and sign in to connectors again.",
        home.display(),
        files + 1
    );
    Ok(())
}

fn prepare_destination(source: &Path, destination: &Path) -> Result<PathBuf> {
    let source = source
        .canonicalize()
        .context("could not resolve the source directory")?;
    std::fs::create_dir_all(destination)?;
    let dest = destination.canonicalize()?;
    if dest.starts_with(&source) {
        bail!("destination must be outside the source directory");
    }
    if std::fs::read_dir(&dest)?.next().transpose()?.is_some() {
        bail!("{} is not empty; choose a new directory", dest.display());
    }
    Ok(dest)
}

fn publish(stage: tempfile::TempDir, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    std::fs::remove_dir(destination)?;
    std::fs::rename(stage.path(), destination).context("could not publish the completed home")
}

/// Copy everything under `from` into `to`, skipping `SKIP` at the top level and any `*.lock`.
fn copy_tree(from: &Path, to: &Path) -> Result<usize> {
    let mut copied = 0;
    let mut stack: Vec<PathBuf> = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in std::fs::read_dir(from.join(&rel))? {
            let entry = entry?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if (rel.as_os_str().is_empty()
                && (SKIP.contains(&name_str.as_ref()) || name_str.starts_with(".jeeves-config-")))
                || name_str.ends_with(".lock")
            {
                continue;
            }
            let child = rel.join(&name);
            let kind = entry.file_type()?;
            if kind.is_dir() {
                std::fs::create_dir_all(to.join(&child))?;
                stack.push(child);
            } else if kind.is_file() {
                std::fs::copy(from.join(&child), to.join(&child))?;
                copied += 1;
            }
        }
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_home_survives_export_and_import_without_its_secrets() {
        let root = tempfile::tempdir().expect("temporary directory");
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join("skills/demo")).unwrap();
        std::fs::write(home.join("SOUL.md"), "be brief").unwrap();
        std::fs::write(home.join("skills/demo/SKILL.md"), "---\nname: demo\n---").unwrap();
        std::fs::write(home.join("secrets.json"), "{\"ds\":\"sk-secret\"}").unwrap();
        std::fs::write(
            home.join(jeeves_config::models::MODEL_CHANGE_FILE),
            "pending",
        )
        .unwrap();
        std::fs::write(home.join(".jeeves-config-fixture"), "unpublished").unwrap();
        std::fs::write(home.join("app-server.lock"), "").unwrap();
        let pool = jeeves_state::open(&home.join("state.db")).await.unwrap();
        jeeves_state::Chat::new(pool.clone())
            .append_exchange("hello", "hi", "main", "cli", None)
            .await
            .unwrap();
        pool.close().await;

        let bundle = root.path().join("bundle");
        export(&home, &bundle).await.expect("export");
        assert!(!bundle.join("secrets.json").exists(), "keys never leave");
        assert!(!bundle.join("app-server.lock").exists());
        assert!(!bundle
            .join(jeeves_config::models::MODEL_CHANGE_FILE)
            .exists());
        assert!(!bundle.join(".jeeves-config-fixture").exists());
        assert!(export(&home, &home.join("backup"))
            .await
            .unwrap_err()
            .to_string()
            .contains("outside"));

        let manifest_path = bundle.join(MANIFEST);
        let manifest = std::fs::read_to_string(&manifest_path).unwrap();
        let mut wrong: serde_json::Value = serde_json::from_str(&manifest).unwrap();
        wrong["schema"] = json!(0);
        std::fs::write(&manifest_path, wrong.to_string()).unwrap();
        let refused = root.path().join("refused");
        assert!(import(&refused, &bundle)
            .await
            .unwrap_err()
            .to_string()
            .contains("differs"));
        assert!(!refused.join("SOUL.md").exists());
        std::fs::write(manifest_path, manifest).unwrap();

        let moved = root.path().join("moved");
        import(&moved, &bundle).await.expect("import");
        assert_eq!(
            std::fs::read_to_string(moved.join("SOUL.md")).unwrap(),
            "be brief"
        );
        assert!(moved.join("skills/demo/SKILL.md").exists());
        let pool = jeeves_state::open(&moved.join("state.db")).await.unwrap();
        let history = jeeves_state::Chat::new(pool)
            .session_history("main")
            .await
            .unwrap();
        assert_eq!(history.len(), 1, "the conversation came along");

        assert!(
            import(&moved, &bundle).await.is_err(),
            "never over a home in use"
        );
    }

    #[tokio::test]
    async fn review_import_requires_a_real_schema_and_database_before_copying_files() {
        let root = tempfile::tempdir().unwrap();
        let bundle = root.path().join("bundle");
        let home = root.path().join("moved");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::write(bundle.join("note"), "fixture").unwrap();
        std::fs::write(bundle.join(MANIFEST), r#"{"format":1}"#).unwrap();
        let error = import(&home, &bundle).await.unwrap_err();
        assert!(error.to_string().contains("schema"), "{error}");
        assert!(!home.join("note").exists());
    }
}
