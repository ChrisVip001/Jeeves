//! Durable device grants. Each change and its generation check share a SQLite statement.
use sqlx::{FromRow, SqliteConnection, SqlitePool};

#[derive(FromRow)]
pub struct DeviceGrant {
    pub device_id: String,
    pub grant_id: String,
    pub generation: i32,
    pub capabilities: String,
    pub revoked: bool,
    pub last_sequence: i32,
    pub push_sequence: Option<i32>,
}

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<DeviceGrant>> {
    sqlx::query_as("SELECT * FROM device_grants ORDER BY device_id")
        .fetch_all(pool)
        .await
}

pub async fn pair(
    conn: &mut SqliteConnection,
    id: &str,
    expected: i32,
    capabilities: &str,
) -> sqlx::Result<Option<DeviceGrant>> {
    if expected == 0 {
        sqlx::query_as("INSERT INTO device_grants(device_id,grant_id,generation,capabilities) VALUES (?,lower(hex(randomblob(16))),1,?) ON CONFLICT(device_id) DO NOTHING RETURNING *")
            .bind(id).bind(capabilities).fetch_optional(conn).await
    } else {
        sqlx::query_as("UPDATE device_grants SET grant_id=lower(hex(randomblob(16))), generation=generation+1, capabilities=?, revoked=0, push_sequence=NULL, last_sequence=0 WHERE device_id=? AND generation=? RETURNING *")
            .bind(capabilities).bind(id).bind(expected).fetch_optional(conn).await
    }
}

pub async fn revoke(
    conn: &mut SqliteConnection,
    id: &str,
    expected: i32,
) -> sqlx::Result<Option<DeviceGrant>> {
    sqlx::query_as("UPDATE device_grants SET revoked=1,push_sequence=NULL,generation=generation+1 WHERE device_id=? AND generation=? AND revoked=0 RETURNING *")
        .bind(id).bind(expected).fetch_optional(conn).await
}

pub async fn report(
    pool: &SqlitePool,
    id: &str,
    grant: &str,
    generation: i32,
    capability: &str,
    sequence: i32,
) -> sqlx::Result<bool> {
    let result = sqlx::query("UPDATE device_grants SET last_sequence=? WHERE device_id=? AND grant_id=? AND generation=? AND revoked=0 AND last_sequence<? AND EXISTS (SELECT 1 FROM json_each(device_grants.capabilities) WHERE value=json_extract(?,'$'))")
        .bind(sequence).bind(id).bind(grant).bind(generation).bind(sequence).bind(capability).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}
