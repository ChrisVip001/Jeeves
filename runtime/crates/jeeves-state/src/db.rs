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

    let applied = applied_version(&pool).await?;
    let target = known_schema_version();
    if applied > target && !matches!(applied, 24 | 25) {
        anyhow::bail!("database schema {applied} is newer than supported schema {target}");
    }
    if applied > 0 && applied != target {
        backup_before_upgrade(&pool, &absolute, target).await?;
    }
    if matches!(applied, 24 | 25) {
        adopt_incremental_baseline(&pool).await?;
    }
    MIGRATOR.run(&pool).await.context("migrations failed")?;
    Ok(pool)
}

/// Where the copy taken before migrating to `version` lives: `state.db.pre-<version>`.
pub fn backup_path(db: &Path, version: i64) -> PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(format!(".pre-{version}"));
    PathBuf::from(name)
}

async fn backup_before_upgrade(pool: &SqlitePool, db: &Path, target: i64) -> Result<()> {
    let dest = backup_path(db, target);
    if dest.try_exists()? {
        return Ok(());
    }
    let staging =
        tempfile::NamedTempFile::new_in(db.parent().context("missing database directory")?)?;
    snapshot(pool, staging.path()).await?;
    match staging.persist_noclobber(&dest) {
        Ok(_) => eprintln!(
            "(jeeves) pre-migration snapshot saved at {}",
            dest.display()
        ),
        Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

/// Adopt the two released incremental histories whose final schema matches the squashed baseline.
async fn adopt_incremental_baseline(pool: &SqlitePool) -> Result<()> {
    use sha2::{Digest, Sha256};
    let baseline = MIGRATOR
        .iter()
        .find(|m| m.version == 1)
        .context("missing baseline")?;
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let rows: Vec<(i64, bool, Vec<u8>)> =
        sqlx::query_as("SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *tx)
            .await?;
    if rows.first().is_some_and(|(v, ok, hash)| {
        *v == 1 && *ok && hash.as_slice() == baseline.checksum.as_ref()
    }) {
        tx.commit().await?;
        return Ok(());
    }
    let mut digest = Sha256::new();
    for (i, (version, ok, checksum)) in rows.iter().enumerate() {
        anyhow::ensure!(
            *version == i as i64 + 1 && *ok && checksum.len() == 48,
            "Incomplete incremental migration history"
        );
        digest.update(checksum);
    }
    let fingerprint = format!("{:x}", digest.finalize());
    anyhow::ensure!(matches!((rows.len(),fingerprint.as_str()),
        (24,"ac39a792c93e6ad22d026cd7adf213840c40ee7456406c0ae17fde6dd30f3509") |
        (25,"489d2a9dafb6c1b1a667ad705ed8d26f0d8ac0bcc2acfca966d9a11f2ce39bb4")
    ), "Unrecognized incremental schema history; preserve this database and use the matching Jeeves release");
    if rows.len() == 24 {
        sqlx::query("CREATE TABLE historical_session_origins AS SELECT id,forked_from FROM sessions WHERE forked_from IS NOT NULL").execute(&mut *tx).await?;
        sqlx::query("ALTER TABLE sessions DROP COLUMN forked_from")
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("CREATE TABLE historical_schema_migrations AS SELECT * FROM _sqlx_migrations")
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version<>1")
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE _sqlx_migrations SET checksum=?,description=? WHERE version=1")
        .bind(baseline.checksum.as_ref())
        .bind(baseline.description.as_ref())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
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
