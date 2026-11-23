//! Human-approved files are published under Runtime-owned directories. The page never supplies a path.
use super::{BrowserHost, Session, store};
use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use jeeves_protocol::*;
use sqlx::{FromRow, SqlitePool};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const MAX_BYTES: usize = 768 * 1024;
#[derive(FromRow)]
struct Accepted {
    session_id: String,
    download_id: String,
    task_id: String,
    name: String,
    body: Vec<u8>,
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}
fn filename(id: &str, name: &str) -> Result<String> {
    ensure!(valid_id(id), "Invalid browser download ID");
    let mut leaf = format!("{id}-");
    for c in name.chars() {
        if leaf.len() + c.len_utf8() > 190 {
            break;
        }
        leaf.push(if c.is_control() || matches!(c, '/' | '\\' | ':') {
            '_'
        } else {
            c
        });
    }
    Ok(leaf)
}

fn publish(home: &Path, accepted: &Accepted) -> Result<PathBuf> {
    ensure!(
        Path::new(&accepted.task_id)
            .file_name()
            .is_some_and(|s| s == accepted.task_id.as_str())
            && !accepted.task_id.contains('\\'),
        "Invalid task artifact directory"
    );
    let leaf = filename(&accepted.download_id, &accepted.name)?;
    let mut dir = home.to_owned();
    for part in ["artifacts", accepted.task_id.as_str()] {
        dir.push(part);
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ensure!(
                std::fs::symlink_metadata(&dir)?.is_dir(),
                "Artifact directory changed"
            ),
            Err(e) => return Err(e.into()),
        }
    }
    let path = dir.join(leaf);
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    tmp.write_all(&accepted.body)?;
    tmp.as_file().sync_all()?;
    match tmp.persist_noclobber(&path) {
        Ok(_) => {}
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure!(
                std::fs::symlink_metadata(&path)?.is_file(),
                "Artifact path is not a regular file"
            );
            let mut existing = Vec::new();
            std::fs::File::open(&path)?
                .take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut existing)?;
            ensure!(
                existing == accepted.body,
                "Existing download differs; kept for review"
            );
        }
        Err(e) => return Err(e.error.into()),
    }
    #[cfg(unix)]
    std::fs::File::open(&dir)?.sync_all()?;
    Ok(path)
}

async fn deliver(pool: &SqlitePool, home: &Path, accepted: Accepted) -> Result<()> {
    let id = accepted.download_id.clone();
    let session = accepted.session_id.clone();
    let task = accepted.task_id.clone();
    let bytes = accepted.body.len() as i64;
    let home = home.to_owned();
    let path = tokio::task::spawn_blocking(move || publish(&home, &accepted)).await??;
    jeeves_state::Artifacts::new(pool.clone())
        .add(&task, None, &path.to_string_lossy(), bytes)
        .await?;
    sqlx::query("UPDATE browser_downloads SET body=NULL WHERE session_id=? AND download_id=?")
        .bind(session)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub(crate) async fn recover(pool: &SqlitePool, home: &Path) -> Result<()> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT session_id,download_id FROM browser_downloads WHERE body IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    for (session, id) in rows {
        if let Some(row) = pending(pool, &session, &id).await?
            && let Err(error) = deliver(pool, home, row).await
        {
            eprintln!("(jeeves) accepted browser download {id} still needs delivery: {error}");
        }
    }
    Ok(())
}

async fn pending(pool: &SqlitePool, session: &str, id: &str) -> Result<Option<Accepted>> {
    Ok(sqlx::query_as(
        "SELECT * FROM browser_downloads WHERE session_id=? AND download_id=? AND body IS NOT NULL",
    )
    .bind(session)
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

impl BrowserHost {
    async fn download_view(
        &self,
        session: &Session,
        mut rows: Vec<BrowserDownloadView>,
    ) -> Result<BrowserDownloadsResponse> {
        let saved: Vec<(String, bool)> = sqlx::query_as(
            "SELECT download_id,body IS NULL FROM browser_downloads WHERE session_id=?",
        )
        .bind(&session.id)
        .fetch_all(&self.pool)
        .await?;
        for row in &mut rows {
            if let Some((_, delivered)) = saved.iter().find(|(id, _)| id == &row.id) {
                if *delivered {
                    row.status = BrowserDownloadStatus::Saved;
                    row.detail = None;
                } else {
                    row.detail = Some("Accepted file awaits artifact delivery; retry save".into());
                }
            }
        }
        Ok(BrowserDownloadsResponse { downloads: rows })
    }
    pub(crate) async fn downloads(&self, task: &str) -> Result<BrowserDownloadsResponse> {
        let session = self.session(task)?;
        self.require_remote_worker(&session)?;
        let reply = self
            .exchange(&session, BrowserWorkerCommand::Downloads)
            .await?;
        self.download_view(&session, download_rows(reply)?).await
    }
    pub(crate) async fn decide_download(
        &self,
        server: &crate::Server,
        p: BrowserDownloadParams,
    ) -> Result<BrowserDownloadsResponse> {
        let session = self.session(&p.task_id)?;
        let _control = session.control.lock().await;
        self.human_authority(&session, p.generation).await?;
        ensure!(valid_id(&p.download_id), "Invalid browser download ID");
        let home = server.settings().home;
        let accepted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM browser_downloads WHERE session_id=? AND download_id=?)",
        )
        .bind(&session.id)
        .bind(&p.download_id)
        .fetch_one(&self.pool)
        .await?;
        if accepted {
            ensure!(
                p.action == BrowserDownloadAction::Save,
                "This file was already accepted"
            );
            if let Some(row) = pending(&self.pool, &session.id, &p.download_id).await? {
                deliver(&self.pool, &home, row).await?;
            }
            return self.downloads(&p.task_id).await;
        }
        let mut reply = self
            .exchange(
                &session,
                BrowserWorkerCommand::Download {
                    generation: p.generation,
                    download_id: p.download_id.clone(),
                    action: p.action,
                },
            )
            .await?;
        let encoded = reply.download_data.take();
        let rows = download_rows(reply)?;
        if p.action == BrowserDownloadAction::Save {
            let encoded = encoded.context("Browser did not provide the selected file")?;
            ensure!(
                encoded.len() <= MAX_BYTES.div_ceil(3) * 4,
                "Download exceeds 768 KiB"
            );
            let body = STANDARD.decode(encoded)?;
            let row = rows
                .iter()
                .find(|row| row.id == p.download_id && row.status == BrowserDownloadStatus::Pending)
                .context("Selected file is not ready")?;
            filename(&row.id, &row.name)?;
            let admission = self.executor_gate.lock().await;
            self.admitted_generation(*admission).await?;
            let inserted=sqlx::query("INSERT INTO browser_downloads(session_id,download_id,task_id,name,body)
                SELECT id,?,task_id,?,? FROM browser_sessions WHERE id=? AND task_id=? AND generation=? AND status='human'")
                .bind(&row.id).bind(&row.name).bind(&body).bind(&session.id).bind(&session.task).bind(p.generation)
                .execute(&self.pool).await?.rows_affected();
            ensure!(
                inserted == 1,
                "Browser control changed before the download was accepted"
            );
            drop(admission);
            deliver(
                &self.pool,
                &home,
                Accepted {
                    session_id: session.id.clone(),
                    download_id: row.id.clone(),
                    task_id: session.task.clone(),
                    name: row.name.clone(),
                    body,
                },
            )
            .await?;
        }
        self.download_view(&session, rows).await
    }
    fn require_remote_worker(&self, session: &Session) -> Result<()> {
        ensure!(
            session.worker.remote_supported(),
            "Refresh the installed Jeeves browser components to use remote control and downloads"
        );
        Ok(())
    }
    async fn human_authority(&self, session: &Arc<Session>, generation: i32) -> Result<()> {
        self.require_remote_worker(session)?;
        let row = store::get(&self.pool, &session.task)
            .await?
            .context("Browser session missing")?;
        ensure!(
            row.id == session.id && row.generation == generation && row.status == "human",
            "Take control and refresh the task browser before acting"
        );
        Ok(())
    }
    pub(crate) async fn human_input(
        &self,
        p: BrowserHumanInputParams,
    ) -> Result<BrowserFrameResponse> {
        let session = self.session(&p.task_id)?;
        self.human_authority(&session, p.generation).await?;
        ensure!(valid_id(&p.frame_id), "Refresh the human browser frame");
        if let BrowserHumanInput::Text { text } = &p.input {
            ensure!(
                !text.is_empty() && text.encode_utf16().count() <= 4000,
                "Invalid input length"
            );
        }
        let reply = self
            .exchange(
                &session,
                BrowserWorkerCommand::HumanInput {
                    generation: p.generation,
                    frame_id: p.frame_id,
                    input: p.input,
                },
            )
            .await?;
        Self::preview(reply)
    }
}
fn download_rows(reply: BrowserWorkerResponse) -> Result<Vec<BrowserDownloadView>> {
    if let Some(error) = reply.error {
        return Err(anyhow!(error));
    }
    reply
        .downloads
        .context("Browser did not return its download list")
}
