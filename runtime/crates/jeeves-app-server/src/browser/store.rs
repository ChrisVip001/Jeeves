use anyhow::{ensure, Result};
use jeeves_protocol::BrowserSessionView;
use sqlx::{FromRow, SqlitePool};
#[derive(Clone, FromRow)]
pub(super) struct Row {
    pub id: String,
    pub task_id: String,
    pub generation: i32,
    pub status: String,
    pub origins_json: String,
    pub url: String,
    pub title: String,
    pub observed_at: Option<String>,
    pub error: Option<String>,
}
impl Row {
    pub fn view(self) -> Result<BrowserSessionView> {
        Ok(BrowserSessionView {
            id: self.id,
            task_id: self.task_id,
            status: self.status,
            generation: self.generation,
            origins: serde_json::from_str(&self.origins_json)?,
            url: self.url,
            title: self.title,
            observed_at: self.observed_at,
            error: self.error,
        })
    }
}
pub(super) async fn get(pool: &SqlitePool, task: &str) -> Result<Option<Row>> {
    Ok(
        sqlx::query_as("SELECT * FROM browser_sessions WHERE task_id=?")
            .bind(task)
            .fetch_optional(pool)
            .await?,
    )
}
pub(super) async fn list(pool: &SqlitePool, owner: &str) -> Result<Vec<Row>> {
    Ok(sqlx::query_as("SELECT * FROM browser_sessions WHERE owner_session_id=? ORDER BY updated_at DESC LIMIT 100").bind(owner).fetch_all(pool).await?)
}
pub(super) async fn create(
    pool: &SqlitePool,
    task: &str,
    owner: &str,
    origins: &[String],
    url: &str,
) -> Result<Row> {
    sqlx::query("INSERT INTO browser_sessions(id,task_id,owner_session_id,status,origins_json,url) VALUES(lower(hex(randomblob(16))),?,?,'starting',?,?) ON CONFLICT(task_id) DO UPDATE SET id=lower(hex(randomblob(16))),generation=generation+1,status='starting',origins_json=excluded.origins_json,url=excluded.url,title='',observed_at=NULL,error=NULL,updated_at=datetime('now') WHERE browser_sessions.status IN ('closed','needsAttention','starting')").bind(task).bind(owner).bind(serde_json::to_string(origins)?).bind(url).execute(pool).await?;
    let row = get(pool, task)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Browser session missing"))?;
    ensure!(
        row.status == "starting",
        "Close the existing browser before starting another"
    );
    Ok(row)
}
pub(super) async fn update(pool: &SqlitePool, row: &Row, expected: i32) -> Result<bool> {
    let changed = sqlx::query("UPDATE browser_sessions SET generation=?,status=?,url=?,title=?,observed_at=?,error=?,updated_at=datetime('now')
        WHERE task_id=? AND id=? AND generation=? AND status NOT IN ('closed','needsAttention')")
        .bind(row.generation).bind(&row.status).bind(&row.url).bind(&row.title).bind(&row.observed_at).bind(&row.error)
        .bind(&row.task_id).bind(&row.id).bind(expected).execute(pool).await?.rows_affected();
    Ok(changed == 1)
}
/// A takeover that outlived the Runtime. Kept until a fresh browser has been opened and observed.
#[derive(Clone, FromRow)]
pub(super) struct Hold {
    pub url: String,
}
pub(super) async fn hold_get(pool: &SqlitePool, task: &str) -> Result<Option<Hold>> {
    Ok(
        sqlx::query_as("SELECT url FROM browser_holds WHERE task_id=?")
            .bind(task)
            .fetch_optional(pool)
            .await?,
    )
}
pub(super) async fn hold_clear(
    pool: &SqlitePool,
    task: &str,
    session: &str,
    generation: i32,
) -> Result<()> {
    let changed = sqlx::query(
        "DELETE FROM browser_holds WHERE task_id=? AND EXISTS (
        SELECT 1 FROM browser_sessions WHERE id=? AND task_id=browser_holds.task_id
        AND generation=? AND status='agent' AND observed_at IS NOT NULL)",
    )
    .bind(task)
    .bind(session)
    .bind(generation)
    .execute(pool)
    .await?
    .rows_affected();
    ensure!(
        changed == 1,
        "Browser control changed before the fresh observation"
    );
    Ok(())
}
/// Capture every takeover still open at boot (before the sessions are marked ended). An existing
/// hold keeps its original `since`, so a second restart does not make it look new.
pub(super) async fn hold_capture(pool: &SqlitePool) -> Result<Vec<String>> {
    sqlx::query("INSERT INTO browser_holds(task_id,url,title,origins_json) SELECT task_id,url,title,origins_json FROM browser_sessions WHERE status='human' ON CONFLICT(task_id) DO UPDATE SET url=excluded.url,title=excluded.title,origins_json=excluded.origins_json")
        .execute(pool)
        .await?;
    Ok(sqlx::query_scalar("SELECT task_id FROM browser_holds")
        .fetch_all(pool)
        .await?)
}
/// Whether a takeover is still unresolved for this task: automatic recovery must not resume it.
pub(crate) async fn is_held(pool: &SqlitePool, task: &str) -> Result<bool> {
    Ok(hold_get(pool, task).await?.is_some())
}
/// Origin only: a page URL can carry a token in its query, and this text reaches the UI.
pub(super) fn origin_of(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_default()
}
