//! Long-lived human entry; task execution retains its own context and permissions.
mod decision;
pub(crate) mod receipt;
pub(crate) mod task_tool;
#[cfg(test)]
mod tests;
use crate::{EventSink, Server};
use jeeves_protocol::*;
use std::time::Instant;
pub(crate) const MAIN_SESSION: &str = "jeeves-main";
pub(super) fn invalid(message: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: message.to_string(),
        data: None,
    }
}
pub(super) fn internal(error: impl ToString) -> ErrorObject {
    ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: error.to_string(),
        data: None,
    }
}

pub(crate) async fn main_session(
    server: &Server,
    sink: &EventSink,
) -> Result<SessionNewResponse, ErrorObject> {
    let lock = server.session_write_lock("main-session-init");
    let _guard = lock.lock().await;
    let existed = server
        .chat
        .session(MAIN_SESSION)
        .await
        .map_err(internal)?
        .is_some();
    server
        .chat
        .create_session(MAIN_SESSION, Some("Jeeves"), None)
        .await
        .map_err(internal)?;
    if !existed {
        server
            .set_permission(
                SessionPermissionSetParams {
                    session_id: MAIN_SESSION.into(),
                    mode: PermissionMode::Standard,
                    expected_revision: 0,
                    confirm_full_access: false,
                },
                sink,
            )
            .await?;
    }
    Ok(SessionNewResponse {
        session_id: MAIN_SESSION.into(),
    })
}

pub(crate) async fn start(
    server: &Server,
    p: InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    if p.session_id != MAIN_SESSION
        || p.message.trim().is_empty()
        || p.message.len() > 64_000
        || p.message_id.is_empty()
        || p.message_id.len() > 120
        || !p
            .message_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(invalid("Invalid main interaction"));
    }
    if server
        .chat
        .session(&p.session_id)
        .await
        .map_err(internal)?
        .is_none()
    {
        return Err(invalid("Open the main conversation first"));
    }
    let input = serde_json::to_string(&p).map_err(internal)?;
    let lock = server.session_write_lock(&format!("interaction:{}", p.session_id));
    let _guard = lock.lock().await;
    if !jeeves_state::interactions::claim(&server.pool, &p.session_id, &p.message_id, &input)
        .await
        .map_err(internal)?
    {
        return receipt::replay(server, (&p.session_id, &p.message_id), &input, id, sink).await;
    }
    let result = execute(server, &p, id, sink).await;
    if let Err(error) = &result {
        if let Err(failure) = jeeves_state::interactions::fail(
            &server.pool,
            &p.session_id,
            &p.message_id,
            &error.message,
        )
        .await
        {
            eprintln!("(jeeves) interaction failure receipt unavailable: {failure}");
        }
    }
    result
}
/// `turn/start` with a `requestId`: the same message delivered twice runs one turn. The
/// receipt shares the interaction table under a `turn:` key, so the two entries never collide.
pub(crate) async fn start_once(
    server: &Server,
    params: TurnStartParams,
    request: String,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    if request.is_empty()
        || request.len() > 120
        || !request
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(invalid(
            "requestId must be 1-120 characters of letters, digits, '-' or '_'",
        ));
    }
    let session = params
        .session_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "default".to_string());
    let key = format!("turn:{request}");
    let input = serde_json::to_string(&params).map_err(internal)?;
    if !jeeves_state::interactions::claim(&server.pool, &session, &key, &input)
        .await
        .map_err(internal)?
    {
        eprintln!("(jeeves) turn/start {request} was already received; replaying its result");
        return receipt::replay(server, (&session, &key), &input, id, sink).await;
    }
    let result = receipt::recorded(server, (&session, &key), params, id, sink).await;
    if let Err(error) = &result {
        if let Err(failure) =
            jeeves_state::interactions::fail(&server.pool, &session, &key, &error.message).await
        {
            eprintln!("(jeeves) turn receipt failure unavailable: {failure}");
        }
    }
    result
}

/// One human message. An explicit `task_id` is a direct follow-up to that task; anything else is an
/// ordinary turn of the main conversation, which may call the `task` tool to hand work off.
async fn execute(
    server: &Server,
    p: &InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    let may_dispatch = may_dispatch(&server.settings().dispatchers, p.source.as_ref());
    if let Some(reply) = decision::answer(server, p, may_dispatch).await? {
        return receipt::acknowledge(server, p, id, sink, reply, Instant::now()).await;
    }
    if p.task_id.is_some() && !may_dispatch {
        return Err(invalid("Sender may not dispatch task work"));
    }
    let text = p.message.trim().trim_end_matches(['。', '.', '!', '！']);
    let command = [
        "刚才那个先别发，改成草稿",
        "先别发，改成草稿",
        "改成草稿",
        "hold the send and save a draft",
        "Hold off on sending that one. Make it a draft",
    ];
    if may_dispatch
        && command.iter().any(|cmd| {
            text == *cmd
                || text.strip_suffix(cmd).is_some_and(|prefix| {
                    prefix.trim().starts_with("mail-") && prefix.split_whitespace().count() == 1
                })
        })
    {
        if p.source.is_some() {
            return Err(invalid(
                "Use the private main chat to review a mail envelope",
            ));
        }
        let reply = jeeves_state::mail_actions::draft_for_interaction(
            &server.pool,
            &p.session_id,
            p.task_id.as_deref(),
            Some(&p.message_id),
            text,
        )
        .await
        .map_err(internal)?;
        return receipt::acknowledge(server, p, id, sink, reply, Instant::now()).await;
    }
    if let Some(task_id) = &p.task_id {
        let started = Instant::now();
        let task = task_tool::continue_task(server, p, task_id, &p.message, sink, || {})
            .await
            .map_err(invalid)?;
        let reply = format!(
            "Passed your addition to \"{}\"; it keeps using the same workspace.",
            task.title
        );
        return receipt::acknowledge(server, p, id, sink, reply, started).await;
    }
    if server.resolved().is_none() {
        return Err(invalid("Configure a model before sending a message"));
    }
    if !may_dispatch {
        eprintln!("(jeeves) a message from outside the dispatchers gets a conversation without the task tool");
    }
    *server
        .interaction_tool
        .lock()
        .expect("interaction tool lock") =
        may_dispatch.then(|| task_tool::build(server.clone(), p.clone(), sink.clone()));
    let result = receipt::conversation(server, p, id, sink).await;
    *server
        .interaction_tool
        .lock()
        .expect("interaction tool lock") = None;
    result
}

/// Local views (no source) are the owner; a chat sender must be on the dispatcher list.
fn may_dispatch(
    dispatchers: &[String],
    source: Option<&jeeves_protocol::InteractionSource>,
) -> bool {
    source.is_none_or(|source| dispatchers.contains(&source.sender_id))
}

#[cfg(test)]
mod dispatch_tests {
    use super::may_dispatch;
    use jeeves_protocol::InteractionSource;

    fn from(sender: &str) -> InteractionSource {
        InteractionSource {
            channel: "lark".into(),
            chat_id: "oc".into(),
            message_id: "om".into(),
            sender_id: sender.into(),
        }
    }

    #[test]
    fn only_listed_senders_or_local_views_may_dispatch() {
        let list = vec!["ou_owner".to_string()];
        assert!(may_dispatch(&list, None));
        assert!(may_dispatch(&list, Some(&from("ou_owner"))));
        assert!(!may_dispatch(&list, Some(&from("ou_other"))));
        assert!(
            !may_dispatch(&[], Some(&from("ou_owner"))),
            "an empty list admits nobody"
        );
    }
}
