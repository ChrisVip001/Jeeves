//! One commit binds the original request, source and executable work before any worker is woken.
use super::price_watch::Prepared;
use jeeves_protocol::InteractionStartParams;
use sqlx::SqliteConnection;

pub(crate) struct Assignment<'a> {
    pub request: &'a InteractionStartParams,
    pub instruction: &'a str,
    pub invocation: Option<&'a str>,
    pub context: &'a str,
    pub message: String,
    pub watch: Option<&'a Prepared>,
}

impl Assignment<'_> {
    pub async fn commit(&self, conn: &mut SqliteConnection, id: &str) -> anyhow::Result<()> {
        let p = self.request;
        let operation = operation_id(conn, p, self.invocation).await?;
        let evidence: Vec<String> = if p.source.is_none() {
            sqlx::query_scalar("SELECT o.id FROM tool_operations o JOIN (SELECT turn_id FROM ledger_turns WHERE session_id=? AND turn_id NOT LIKE ? ORDER BY rowid DESC LIMIT 2) l ON l.turn_id=o.turn_id WHERE o.session_id=? AND o.task_id IS NULL AND ((o.output_preview IS NOT NULL AND o.status IN ('succeeded','failed')) OR o.status IN ('reconciled_success','reconciled_failure')) ORDER BY o.rowid DESC LIMIT 32")
                .bind(&p.session_id).bind(format!("{}%",jeeves_protocol::CHANNEL_TURN_PREFIX)).bind(&p.session_id).fetch_all(&mut *conn).await?
        } else {
            Vec::new()
        };
        sqlx::query("INSERT INTO conversation_handoffs(task_id,session_id,message_id,instruction,context,evidence_json,operation_id) VALUES(?,?,?,?,?,?,?)")
            .bind(id).bind(&p.session_id).bind(&p.message_id).bind(self.instruction).bind(self.context).bind(serde_json::to_string(&evidence)?).bind(operation).execute(&mut *conn).await?;
        if let Some(origin) = &p.source {
            jeeves_state::task_sources::insert(
                conn,
                id,
                &jeeves_state::task_sources::TaskSource {
                    channel: origin.channel.clone(),
                    chat_id: origin.chat_id.clone(),
                    message_id: origin.message_id.clone(),
                    sender_id: origin.sender_id.clone(),
                },
            )
            .await?;
        }
        if let Some(watch) = self.watch {
            watch.insert(conn, id).await?;
        } else {
            let attachments = serde_json::to_string(&p.attachments.as_deref().unwrap_or_default())?;
            jeeves_state::dispatch::TaskDispatch::enqueue_on(
                conn,
                id,
                &self.message,
                &attachments,
                Some(&p.message_id),
            )
            .await?;
        }
        Ok(())
    }
}

pub(super) async fn operation_id(
    conn: &mut SqliteConnection,
    p: &InteractionStartParams,
    invocation: Option<&str>,
) -> sqlx::Result<Option<String>> {
    sqlx::query_scalar("SELECT o.id FROM tool_operations o JOIN turn_checkpoints c ON c.turn_id=o.turn_id AND c.round_id=o.round_id WHERE o.session_id=? AND o.turn_id=? AND o.status='intent' AND o.task_id IS NULL AND ?=o.round_id||':'||(c.iterations+1)||':'||o.tool_use_id")
        .bind(&p.session_id).bind(jeeves_protocol::interaction_turn_id(&p.message_id,p.source.is_some())).bind(invocation).fetch_optional(conn).await
}

/// The database commit proves local dispatch even if the process lost the tool reply.
pub(crate) async fn committed_result(
    server: &crate::Server,
    row: &jeeves_state::CheckpointRow,
    call: &str,
    input: &serde_json::Value,
) -> anyhow::Result<Option<String>> {
    let op: Option<String> = sqlx::query_scalar(
        "SELECT id FROM tool_operations WHERE turn_id=? AND round_id=? AND tool_use_id=? AND session_id=?
         AND task_id IS NULL AND status='intent' AND input_json=?",
    )
    .bind(&row.turn_id)
    .bind(&row.round_id)
    .bind(call)
    .bind(&row.session_id)
    .bind(input.to_string())
    .fetch_optional(&server.pool)
    .await?;
    let Some(op) = op else { return Ok(None) };
    match input["action"].as_str() {
        Some("create") => {
            let Some(items) = input["items"].as_array() else { return Ok(None) };
            let rows: Vec<(String, String)> = sqlx::query_as(
                "SELECT task_id,instruction FROM conversation_handoffs WHERE operation_id=? ORDER BY rowid",
            )
            .bind(op)
            .fetch_all(&server.pool)
            .await?;
            let complete = !rows.is_empty() && items.len() == rows.len()
                && items.iter().zip(&rows).all(|(item, (_, instruction))| {
                    item["instruction"].as_str() == Some(instruction.as_str())
                });
            if !complete {
                return Ok(None);
            }
            let ids: Vec<_> = rows.into_iter().map(|(id, _)| id).collect();
            Ok(Some(format!("Committed task handoff: {}", ids.join(", "))))
        }
        Some("amend_price") => Ok(sqlx::query_scalar(
            "SELECT 'Committed amendment: '||task_id||' v'||revision FROM task_amendments WHERE operation_id=?",
        )
        .bind(op)
        .fetch_optional(&server.pool)
        .await?),
        _ => Ok(None),
    }
}
