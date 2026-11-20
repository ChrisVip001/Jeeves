//! Open state.db and migrate it to the latest schema.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

/// Migration scripts are read in at compile time: the binary carries its schema, no runtime file lookup.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open (creating if needed) the state.db at `path`, and run all migrations.
pub async fn open(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("could not create the state directory: {}", parent.display())
            })?;
        }
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let options = SqliteConnectOptions::new()
        .filename(&absolute)
        .create_if_missing(true)
        .busy_timeout(Duration::from_secs(10))
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await
        .with_context(|| format!("could not open database {}", absolute.display()))?;

    guard_and_backup(&pool, &absolute, known_schema_version()).await?;
    MIGRATOR.run(&pool).await.context("migrations failed")?;
    Ok(pool)
}

/// Where the copy taken before migrating to `version` lives: `state.db.pre-<version>`.
pub fn backup_path(db: &Path, version: i64) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(format!(".pre-{version}"));
    PathBuf::from(name)
}

/// For a database that already existed: refuse one written by a newer binary, and copy one
/// that is about to be migrated (once per target version) so an upgrade can be undone.
pub(crate) async fn guard_and_backup(pool: &SqlitePool, db: &Path, known: i64) -> Result<()> {
    let applied = applied_version(pool).await?;
    if applied > known {
        let backup = backup_path(db, applied);
        anyhow::bail!(
            "{} is at schema version {applied}, newer than this binary's {known}; \
             upgrade Jeeves, or restore the backup the newer version took: {}",
            db.display(),
            backup.display()
        );
    }
    if applied == 0 || applied == known {
        return Ok(());
    }
    let backup = backup_path(db, known);
    if backup.exists() {
        return Ok(());
    }
    sqlx::query("VACUUM INTO ?")
        .bind(backup.to_string_lossy().to_string())
        .execute(pool)
        .await
        .with_context(|| format!("could not back up the database to {}", backup.display()))?;
    eprintln!(
        "jeeves: backed up {} (schema {applied}) to {} before migrating to {known}",
        db.display(),
        backup.display()
    );
    Ok(())
}

/// Newest applied migration, or 0 when the migrations table does not exist yet.
async fn applied_version(pool: &SqlitePool) -> Result<i64> {
    let has_table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(pool)
    .await?;
    if has_table.is_none() {
        return Ok(0);
    }
    schema_version(pool).await
}

/// Write a consistent copy of the open database to `dest` (`VACUUM INTO`: safe while other
/// connections write) and return its schema version — the newest migration it carries.
pub async fn snapshot(pool: &SqlitePool, dest: &Path) -> Result<i64> {
    sqlx::query("VACUUM INTO ?")
        .bind(dest.to_string_lossy().to_string())
        .execute(pool)
        .await
        .with_context(|| format!("could not snapshot the database to {}", dest.display()))?;
    schema_version(pool).await
}

/// Inspect an exported database before migration. Opening read-only never creates a fresh store.
pub async fn snapshot_version(path: &Path) -> Result<i64> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(SqliteConnectOptions::new().filename(path).read_only(true))
        .await?;
    let version = schema_version(&pool).await;
    pool.close().await;
    version
}

/// The newest migration applied to this database.
pub async fn schema_version(pool: &SqlitePool) -> Result<i64> {
    let version: Option<i64> = sqlx::query_scalar("SELECT MAX(version) FROM _sqlx_migrations")
        .fetch_one(pool)
        .await?;
    Ok(version.unwrap_or(0))
}

/// The newest migration this binary knows.
pub fn known_schema_version() -> i64 {
    MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0)
}
