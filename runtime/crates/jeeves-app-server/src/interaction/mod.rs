//! Long-lived human entry; task execution retains its own context and permissions.
mod decision;
pub(crate) mod handoff;
pub(crate) mod receipt;
pub(crate) mod task_tool;
#[cfg(test)]
mod tests;
use crate::{EventSink, Server};
use jeeves_protocol::*;
use std::time::Instant;
pub const MAIN_SESSION: &str = "jeeves-main";
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

pub async fn main_session(
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
    start_with_tier(server, p, id, sink, None).await
}

#[derive(serde::Serialize)]
struct TieredInteraction<'a> {
    #[serde(flatten)]
    interaction: &'a InteractionStartParams,
    tier: &'a str,
}

async fn start_with_tier(
    server: &Server,
    p: InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
    tier: Option<String>,
) -> Result<(), ErrorObject> {
    if p.source.is_some() && (p.cwd.is_some() || p.attachments.is_some()) {
        return Err(invalid(
            "A channel cannot supply a local workspace or attachments",
        ));
    }
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
    main_session(server, sink).await?;
    let input = match &tier {
        Some(tier) => serde_json::to_string(&TieredInteraction {
            interaction: &p,
            tier,
        }),
        None => serde_json::to_string(&p),
    }
    .map_err(internal)?;
    let lock = server.session_write_lock(&format!("interaction:{}", p.session_id));
    let _guard = lock.lock().await;
    if !jeeves_state::interactions::claim(&server.pool, &p.session_id, &p.message_id, &input)
        .await
        .map_err(internal)?
    {
        return receipt::replay(server, (&p.session_id, &p.message_id), &input, id, sink).await;
    }
    let result = execute(server, &p, id, sink, tier).await;
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
pub(crate) fn require_main(session: Option<&str>) -> Result<&'static str, ErrorObject> {
    match session.map(str::trim) {
        None | Some("" | MAIN_SESSION) => Ok(MAIN_SESSION),
        _ => Err(invalid("Jeeves has one conversation. Use session/main.")),
    }
}

pub async fn submit_main(
    server: &Server,
    params: TurnStartParams,
    id: RequestId,
    sink: &EventSink,
) -> Result<(), ErrorObject> {
    require_main(params.session_id.as_deref())?;
    let message_id = params
        .request_id
        .map(|request| format!("turn-{request}"))
        .unwrap_or_else(|| {
            format!(
                "turn-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            )
        });
    start_with_tier(
        server,
        InteractionStartParams {
            session_id: MAIN_SESSION.into(),
            message_id,
            message: params.message,
            cwd: params.cwd,
            attachments: params.attachments,
            task_id: None,
            source: None,
        },
        id,
        sink,
        params.tier,
    )
    .await
}

/// One human message. An explicit `task_id` is a direct follow-up to that task; anything else is an
/// ordinary turn of the main conversation, which may call the `task` tool to hand work off.
async fn execute(
    server: &Server,
    p: &InteractionStartParams,
    id: RequestId,
    sink: &EventSink,
    tier: Option<String>,
) -> Result<(), ErrorObject> {
    let settings = server.settings();
    let may_dispatch = may_dispatch(&settings.dispatchers, p.source.as_ref())
        || p.source
            .as_ref()
            .is_some_and(|source| lark_owner(&settings.home, source));
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
        task_tool::owned_task(server, p, task_id)
            .await
            .map_err(invalid)?;
        let replacement: Option<String> = sqlx::query_scalar(
            "SELECT responsibility_id FROM responsibility_predecessors WHERE task_id=?",
        )
        .bind(task_id)
        .fetch_optional(&server.pool)
        .await
        .map_err(internal)?;
        if let Some(replacement) = replacement {
            let reply = format!("Manage this imported assignment in Recurring Work ({replacement}). Its old history is here. Ask in the main chat to review its new sources and conditions.");
            return receipt::acknowledge(server, p, id, sink, reply, started).await;
        }
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
    let tool = may_dispatch.then(|| task_tool::build(server.clone(), p.clone(), sink.clone()));
    receipt::conversation(server, p, id, sink, tool.as_ref(), tier).await
}

fn lark_owner(home: &std::path::Path, source: &InteractionSource) -> bool {
    if source.channel != "lark" {
        return false;
    }
    #[derive(serde::Deserialize)]
    struct LarkAccess {
        enabled: bool,
        allow: Vec<String>,
        #[serde(default)]
        dispatchers: Vec<String>,
    }
    std::fs::read(home.join("gateways.json"))
        .ok()
        .and_then(|data| serde_json::from_slice::<LarkAccess>(&data).ok())
        .is_some_and(|config| {
            config.enabled
                && config.allow.contains(&source.sender_id)
                && (config.allow.len() == 1 || config.dispatchers.contains(&source.sender_id))
        })
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
