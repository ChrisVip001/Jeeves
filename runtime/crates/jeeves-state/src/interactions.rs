//! Human-message admission and receipts. Claimed work is never automatically replayed.
use anyhow::{Result, ensure};
use sqlx::{FromRow, SqlitePool};
#[derive(FromRow)]
pub struct Receipt {
    pub input_json: String,
    pub state: String,
    pub result_json: Option<String>,
    pub events_json: Option<String>,
    pub error: Option<String>,
}
pub async fn claim(pool: &SqlitePool, session: &str, message: &str, input: &str) -> Result<bool> {
    Ok(sqlx::query("INSERT INTO interaction_receipts(session_id,message_id,input_json,state) VALUES(?,?,?,'claimed') ON CONFLICT DO NOTHING")
        .bind(session).bind(message).bind(input).execute(pool).await?.rows_affected()==1)
}
pub async fn get(pool: &SqlitePool, session: &str, message: &str) -> Result<Receipt> {
    Ok(sqlx::query_as("SELECT input_json,state,result_json,events_json,error FROM interaction_receipts WHERE session_id=? AND message_id=?")
        .bind(session).bind(message).fetch_one(pool).await?)
}
pub async fn complete(
    pool: &SqlitePool,
    session: &str,
    message: &str,
    result: &str,
    events: &str,
) -> Result<()> {
    let changed=sqlx::query("UPDATE interaction_receipts SET state='completed',result_json=?,events_json=? WHERE session_id=? AND message_id=? AND state='claimed'")
        .bind(result).bind(events).bind(session).bind(message).execute(pool).await?.rows_affected();
    ensure!(changed == 1, "Human message receipt is no longer claimable");
    Ok(())
}
pub async fn fail(pool: &SqlitePool, session: &str, message: &str, error: &str) -> Result<()> {
    sqlx::query("UPDATE interaction_receipts SET state='failed',error=? WHERE session_id=? AND message_id=? AND state='claimed'")
        .bind(error).bind(session).bind(message).execute(pool).await?;
    Ok(())
}
