//! A task's SMTP envelope and the exclusive right to hand it to the transport.
use anyhow::{Result, ensure};
use sqlx::{FromRow, SqlitePool};

#[derive(FromRow)]
struct Candidate {
    id: String,
    task_id: String,
    title: String,
    envelope: String,
    subject: Option<String>,
    state: String,
}

pub struct MailAction {
    pool: SqlitePool,
    pub id: String,
}

impl MailAction {
    /// Identical task envelopes have one dispatch right, including across runs or recovery.
    pub async fn queue(
        pool: SqlitePool,
        run: &str,
        account: &str,
        envelope: &str,
    ) -> Result<(Self, bool)> {
        let task = crate::Runs::new(pool.clone())
            .get(run)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Unknown mail task run"))?
            .task_id;
        let key = serde_json::to_vec(&(&task, account, envelope))?;
        let id = format!("mail-{}", crate::ledger::digest_of(&key));
        let fresh = sqlx::query("INSERT INTO mail_actions(id,task_id,account,envelope,state) VALUES(?,?,?,?,'queued') ON CONFLICT(id) DO NOTHING")
            .bind(&id).bind(task).bind(account).bind(envelope).execute(&pool).await?.rows_affected() == 1;
        Ok((Self { pool, id }, fresh))
    }

    /// New turns can deliberately send identical mail after a known outcome. An unsettled
    /// envelope or a replayed invocation keeps its original dispatch right.
    pub async fn standalone(
        pool: SqlitePool,
        session: &str,
        call: &str,
        account: &str,
        envelope: &str,
    ) -> Result<(Self, bool)> {
        let key = serde_json::to_vec(&(session, call))?;
        let id = format!("chat-mail-{}", crate::ledger::digest_of(&key));
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous:Option<String>=sqlx::query_scalar("SELECT id FROM mail_actions WHERE id=? OR (session_id=? AND account=? AND envelope=? AND state IN ('queued','started','unknown')) LIMIT 1")
            .bind(&id).bind(session).bind(account).bind(envelope).fetch_optional(&mut *tx).await?;
        if let Some(id) = previous {
            tx.commit().await?;
            return Ok((Self { pool, id }, false));
        }
        sqlx::query("INSERT INTO mail_actions(id,session_id,account,envelope,state) VALUES(?,?,?,?,'queued')")
            .bind(&id).bind(session).bind(account).bind(envelope).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok((Self { pool, id }, true))
    }

    pub async fn state(&self) -> Result<String> {
        Ok(
            sqlx::query_scalar("SELECT state FROM mail_actions WHERE id=?")
                .bind(&self.id)
                .fetch_one(&self.pool)
                .await?,
        )
    }

    pub async fn claim(&self) -> Result<bool> {
        self.change("queued", "started").await
    }

    pub async fn deny(&self) -> Result<()> {
        self.change("queued", "unapproved").await?;
        Ok(())
    }

    pub async fn settle(&self, outcome: &str) -> Result<()> {
        ensure!(
            self.change("started", outcome).await?,
            "SMTP action lost its claim"
        );
        Ok(())
    }

    async fn change(&self, from: &str, to: &str) -> Result<bool> {
        Ok(
            sqlx::query("UPDATE mail_actions SET state=? WHERE id=? AND state=?")
                .bind(to)
                .bind(&self.id)
                .bind(from)
                .execute(&self.pool)
                .await?
                .rows_affected()
                == 1,
        )
    }
}

fn referenced<'a>(scope: &[&'a Candidate], message: &str) -> Option<Vec<&'a Candidate>> {
    let words: Vec<_> = message
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .collect();
    if words.iter().any(|w| w.starts_with("mail-")) {
        return Some(
            scope
                .iter()
                .copied()
                .filter(|a| words.contains(&a.id.as_str()))
                .collect(),
        );
    }
    let named: Vec<_> = scope
        .iter()
        .copied()
        .filter(|a| {
            words.contains(&a.task_id.as_str())
                || [Some(a.title.as_str()), a.subject.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|name| !name.is_empty() && message.contains(name))
        })
        .collect();
    (!named.is_empty()).then_some(named)
}

/// Amend using only the supplied message and task binding.
pub async fn draft(
    pool: &SqlitePool,
    session: &str,
    bound_task: Option<&str>,
    message: &str,
) -> Result<String> {
    draft_for_interaction(pool, session, bound_task, None, message).await
}

/// Only human-supplied references narrow the set. Never use a model-selected ID as evidence.
/// Holding the SQLite writer lock makes target resolution and conversion indivisible with claim.
pub async fn draft_for_interaction(
    pool: &SqlitePool,
    session: &str,
    bound_task: Option<&str>,
    message_id: Option<&str>,
    message: &str,
) -> Result<String> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let all: Vec<Candidate> = sqlx::query_as(
        "SELECT a.id,a.task_id,t.title,a.envelope,a.state,
                CASE WHEN json_valid(a.envelope) THEN json_extract(a.envelope,'$.subject') END AS subject
         FROM mail_actions a JOIN agent_tasks t ON t.id=a.task_id
         WHERE t.session_id=? AND (? IS NULL OR a.task_id=?) ORDER BY a.task_id,a.id"
    ).bind(session).bind(bound_task).bind(bound_task).fetch_all(&mut *tx).await?;
    let mut scope: Vec<_> = all.iter().collect();
    let mut target = referenced(&scope, message);
    let text = message.to_ascii_lowercase();
    let refers_back = [
        "刚才",
        "那封",
        "that one",
        "that email",
        "that mail",
        "previous email",
    ]
    .iter()
    .any(|phrase| text.contains(phrase));
    if target.is_none() && bound_task.is_none() && refers_back {
        let previous: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT json_extract(input_json,'$.message'), json_extract(input_json,'$.taskId')
             FROM (SELECT state,input_json,message_id FROM interaction_receipts
                   WHERE session_id=? AND rowid < (SELECT rowid FROM interaction_receipts WHERE session_id=? AND message_id=?)
                   ORDER BY rowid DESC LIMIT 1)
             WHERE state='completed' AND json_extract(input_json,'$.messageId')=message_id
               AND json_extract(input_json,'$.source') IS NULL"
        ).bind(session).bind(session).bind(message_id).fetch_optional(&mut *tx).await?;
        if let Some((prior_message, task)) = previous {
            if let Some(task) = task {
                scope.retain(|action| action.task_id == task);
            }
            target = referenced(&scope, &prior_message);
        }
    }
    let candidates = target.unwrap_or(scope);
    let reply = match candidates.as_slice() {
        [] => "此会话中没有匹配的发件动作。 No matching email action.".to_owned(),
        [action] => {
            let status = if action.state == "queued" {
                sqlx::query("UPDATE mail_actions SET state='draft' WHERE id=? AND state='queued'")
                    .bind(&action.id)
                    .execute(&mut *tx)
                    .await?;
                let note = format!(
                    "已保存草稿 / Saved draft · {}\n{}",
                    action.id, action.envelope
                );
                sqlx::query(
                    "INSERT INTO chat_log(session_id,role,source,task_id,notice_key,content)
                     SELECT session_id,'assistant','mail_draft',task_id,?,? FROM task_dispatch WHERE task_id=?"
                ).bind(&action.id).bind(note).bind(&action.task_id).execute(&mut *tx).await?;
                "draft"
            } else {
                action.state.as_str()
            };
            let detail = match status {
                "draft" => format!("已保存草稿 / Saved draft: {}", action.envelope),
                "confirmed" => "SMTP 已确认接收 / SMTP accepted the message.".into(),
                "started" => {
                    "发送器已领取；目前无最终回执 / Sender claimed it; final outcome unconfirmed."
                        .into()
                }
                "unknown" => "结果不明；未重试 / Outcome unknown; no retry.".into(),
                "rejected" => "SMTP 已拒绝 / SMTP rejected the message.".into(),
                "unapproved" => "未获批准，未发送 / Not approved or sent.".into(),
                _ => anyhow::bail!("Unexpected mail action state"),
            };
            format!(
                "{} · {} · {}\n{detail}",
                action.title, action.task_id, action.id
            )
        }
        several => {
            let choices: Vec<_> = several
                .iter()
                .map(|action| {
                    let title = action.subject.as_deref().unwrap_or(&action.title);
                    format!("{title} · {} · {}", action.task_id, action.id)
                })
                .collect();
            format!(
                "请选择发件动作，将动作 ID 连同改为草稿的指令一起回复。 Which email? Reply with its action ID and your draft instruction.\n{}",
                choices.join("\n")
            )
        }
    };
    tx.commit().await?;
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn a_private_reference_survives_until_the_next_message() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::open(&dir.path().join("state.db")).await.unwrap();
        let first = add(&pool, "alpha").await;
        let second = add(&pool, "beta").await;
        for (id, mut input, expected) in [
            (
                "external",
                serde_json::json!({"message":first.id,"source":{"channel":"lark"}}),
                "Which email",
            ),
            (
                "ambiguous",
                serde_json::json!({"message":"alpha and beta"}),
                "Which email",
            ),
            (
                "private",
                serde_json::json!({"message":format!("Review {}",first.id)}),
                "Saved draft",
            ),
        ] {
            input["messageId"] = id.into();
            crate::interactions::claim(&pool, "main", id, &input.to_string())
                .await
                .unwrap();
            crate::interactions::complete(&pool, "main", id, "{}", "[]")
                .await
                .unwrap();
            let next = format!("after-{id}");
            crate::interactions::claim(&pool, "main", &next, "{}")
                .await
                .unwrap();
            let reply =
                draft_for_interaction(&pool, "main", None, Some(&next), "刚才那个先别发，改成草稿")
                    .await
                    .unwrap();
            assert!(reply.contains(expected), "{reply}");
            assert_eq!(second.state().await.unwrap(), "queued");
        }
    }
    async fn add(pool: &SqlitePool, id: &str) -> MailAction {
        crate::Tasks::new(pool.clone())
            .create(crate::NewTask {
                id,
                session_id: Some("main"),
                title: id,
                goal_condition: None,
                goal_draft: None,
                acceptance_json: "[]",
                budget_json: "{}",
                cwd: None,
                max_rounds: 1,
                tier: None,
            })
            .await
            .unwrap();
        crate::Runs::new(pool.clone())
            .start(id, id, None, None)
            .await
            .unwrap();
        MailAction::queue(pool.clone(), id, "fixture", "synthetic envelope")
            .await
            .unwrap()
            .0
    }

    #[tokio::test]
    async fn replayed_chat_call_and_new_approved_call_are_distinct_after_acceptance() {
        let home = tempfile::tempdir().unwrap();
        let pool = crate::open(&home.path().join("state.db")).await.unwrap();
        let queue =
            |call| MailAction::standalone(pool.clone(), "chat", call, "account", "synthetic");
        let (first, fresh) = queue("round-1:call").await.unwrap();
        assert!(fresh && first.claim().await.unwrap());
        assert!(!queue("round-2:call").await.unwrap().1);
        first.settle("confirmed").await.unwrap();
        let (replayed, fresh) = queue("round-1:call").await.unwrap();
        assert!(!fresh);
        assert_eq!(replayed.id, first.id);
        assert!(queue("round-2:call").await.unwrap().1);
    }

    #[tokio::test]
    async fn amendment_scopes_references_and_survives_restart() {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("state.db");
        let pool = crate::open(&db).await.unwrap();
        let first = add(&pool, "alpha").await;
        let second = add(&pool, "beta").await;
        assert!(
            draft(&pool, "main", None, "刚才那个先别发，改成草稿")
                .await
                .unwrap()
                .contains("Which email")
        );
        assert!(
            draft(&pool, "other", None, &first.id)
                .await
                .unwrap()
                .contains("No matching")
        );
        assert!(
            draft(&pool, "main", Some("alpha"), "mail-invalid")
                .await
                .unwrap()
                .contains("No matching")
        );
        assert_eq!(first.state().await.unwrap(), "queued");
        let saved = draft(&pool, "main", None, &first.id).await.unwrap();
        assert!(saved.contains("synthetic envelope"));
        assert!(!first.claim().await.unwrap());
        assert_eq!(second.state().await.unwrap(), "queued");
        pool.close().await;
        let reopened = crate::open(&db).await.unwrap();
        let (old, fresh) =
            MailAction::queue(reopened.clone(), "alpha", "fixture", "synthetic envelope")
                .await
                .unwrap();
        assert!(!fresh);
        assert!(!old.claim().await.unwrap());
        assert_eq!(
            saved,
            draft(&reopened, "main", Some("alpha"), "改成草稿")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn draft_and_task_transcript_commit_together_once() {
        let home = tempfile::tempdir().unwrap();
        let pool = crate::open(&home.path().join("state.db")).await.unwrap();
        let action = add(&pool, "report").await;
        crate::dispatch::TaskDispatch::new(pool.clone())
            .enqueue("report", "send report")
            .await
            .unwrap();
        sqlx::query("CREATE TRIGGER reject_draft BEFORE INSERT ON chat_log WHEN NEW.source='mail_draft' BEGIN SELECT RAISE(ABORT,'fixture'); END")
            .execute(&pool).await.unwrap();
        assert!(draft(&pool, "main", None, &action.id).await.is_err());
        assert_eq!(action.state().await.unwrap(), "queued");
        sqlx::query("DROP TRIGGER reject_draft")
            .execute(&pool)
            .await
            .unwrap();
        for _ in 0..2 {
            assert!(
                draft(&pool, "main", None, &action.id)
                    .await
                    .unwrap()
                    .contains("Saved draft")
            );
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM chat_log WHERE source='mail_draft' AND notice_key=?",
        )
        .bind(&action.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
        assert!(!action.claim().await.unwrap());
    }

    #[tokio::test]
    async fn simultaneous_claim_and_amend_have_one_winner() {
        let home = tempfile::tempdir().unwrap();
        let pool = crate::open(&home.path().join("state.db")).await.unwrap();
        for n in 0..12 {
            let id = format!("task-{n}");
            let action = add(&pool, &id).await;
            let (claim, reply) =
                tokio::join!(action.claim(), draft(&pool, "main", Some(&id), "改成草稿"));
            let claimed = claim.unwrap();
            assert_eq!(reply.unwrap().contains("Saved draft"), !claimed);
            assert_eq!(
                action.state().await.unwrap(),
                if claimed { "started" } else { "draft" }
            );
        }
        for (state, report) in [
            ("confirmed", "SMTP accepted"),
            ("unknown", "Outcome unknown"),
            ("rejected", "SMTP rejected"),
        ] {
            let action = add(&pool, state).await;
            assert!(action.claim().await.unwrap());
            assert!(
                draft(&pool, "main", Some(state), "改成草稿")
                    .await
                    .unwrap()
                    .contains("Sender claimed")
            );
            action.settle(state).await.unwrap();
            assert!(
                draft(&pool, "main", Some(state), "改成草稿")
                    .await
                    .unwrap()
                    .contains(report)
            );
            assert!(!action.claim().await.unwrap());
        }
    }
}
