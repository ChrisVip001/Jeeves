//! Method dispatch: JSON-RPC method → one state operation / one turn.
//!
//! Each method does very little: parse `*Params` → call the corresponding crate → wrap into `*Response`.
//! The turn is the only complex path, and its flow lives in turn.rs — its notification and response
//! ordering must be controlled by itself, so it does not take the "return value" route.

use jeeves_protocol::{
    codes, methods, ApprovalLogParams, ApprovalRespondParams, ApprovalRespondResponse,
    ConfigReadParams, ConfigReadResponse, ConfigWriteParams, ConfigWriteResponse, ErrorObject,
    JsonRpcError, JsonRpcMessage, JsonRpcRequest, JsonRpcResponse, MemoryForgetParams,
    MemoryForgetResponse, MemoryListEpisodesParams, MemoryListEpisodesResponse, MemoryListParams,
    MemoryListResponse, MemoryRememberParams, MemoryRememberResponse, MemorySearchParams,
    MemorySearchResponse, MessageRole, ModelInfo, ModelListParams, ModelListResponse,
    ModelProfilesParams, ModelRemoveParams, ModelRoutesSaveParams, ModelSaveParams, RequestId,
    ScheduleListParams, ScheduleRemoveParams, ScheduleSaveParams, ServerNotification,
    SessionCapabilitiesParams, SessionCapabilitiesResponse, SessionGetParams, SessionGetResponse,
    SessionListParams, SessionListResponse, SessionMessagesParams, SessionMessagesResponse,
    SessionUpdateParams, SessionUpdateResponse, SessionViewParams, TaskActionParams,
    TaskCreateParams, TaskGetParams, TurnFollowUpParams, TurnFollowUpResponse, TurnInterruptParams,
    TurnInterruptResponse, TurnListParams, TurnListResponse, TurnStartParams, TurnSteerParams,
    TurnSteerResponse, TurnSteeredNotification, TurnView, UsageSummaryParams, JSONRPC_VERSION,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{narrow, turn, EventSink, Server};

/// The product of one method: either a response body (wrapped and sent by handle),
/// or a declaration "I send it myself" (turn — it must finish all notifications before the response).
enum Outcome {
    Value(Value),
    SelfSent,
}

/// Handle one request. **A response is always sent out**: business errors go through JSON-RPC's
/// error object; they neither exit the process nor leave the client waiting until timeout.
pub async fn handle(server: &Server, request: JsonRpcRequest, sink: EventSink) {
    let id = request.id.clone();
    let message = match dispatch(server, &request.method, request.params.as_ref(), &id, &sink).await
    {
        Ok(Outcome::Value(result)) => JsonRpcMessage::Response(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result,
        }),
        Ok(Outcome::SelfSent) => return,
        Err(error) => JsonRpcMessage::Error(JsonRpcError {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: Some(id),
            error,
        }),
    };
    sink.response(message);
}

/// Bumped only on a breaking change to the wire contract. A shell declares what it can do;
/// the engine answers which of those it actually supports, so an old client never guesses.
pub(crate) const PROTOCOL_VERSION: i32 = 1;
pub(crate) const SUPPORTED_FEATURES: &[&str] = &[
    "streaming",
    "images",
    "approval",
    "attachments",
    "tasks",
    "memory",
    "tools",
];

fn negotiate(features: Vec<String>) -> (Vec<String>, Vec<String>) {
    features
        .into_iter()
        .partition(|feature| SUPPORTED_FEATURES.contains(&feature.as_str()))
}

async fn dispatch(
    server: &Server,
    method: &str,
    params: Option<&Value>,
    request_id: &RequestId,
    sink: &EventSink,
) -> Result<Outcome, ErrorObject> {
    match method {
        methods::SESSION_CAPABILITIES => {
            let p: SessionCapabilitiesParams = parse(params, method)?;
            let (accepted, unsupported) = negotiate(p.features);
            respond(SessionCapabilitiesResponse {
                protocol_version: PROTOCOL_VERSION,
                accepted,
                unsupported,
            })
        }
        methods::DEVICE_PAIR => {
            respond(crate::devices::pair(server, parse(params, method)?).await?)
        }
        methods::DEVICE_REVOKE => {
            respond(crate::devices::revoke(server, parse(params, method)?).await?)
        }
        methods::DEVICE_LIST => respond(crate::devices::list(&server.pool).await?),
        methods::DEVICE_REPORT => {
            respond(crate::devices::report(&server.pool, parse(params, method)?).await?)
        }
        methods::DESKTOP_STATUS => respond(
            server
                .desktop_status(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_SELECT => respond(
            server
                .desktop_select(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_OBSERVE => respond(
            server
                .desktop_observe(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_ACT => respond(
            server
                .desktop_act(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_TAKEOVER => respond(
            server
                .desktop_takeover(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_MODEL => respond(
            server
                .desktop_model(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_FRAME => respond(
            server
                .desktop_frame(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::DESKTOP_SHARE => respond(
            server
                .desktop_share(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::EXECUTOR_CONTROL => {
            respond(server.browser.executor_status().await.map_err(internal)?)
        }
        methods::EXECUTOR_STOP => respond(server.browser.emergency_stop().await.map_err(internal)?),
        methods::EXECUTOR_RELEASE => {
            let p: jeeves_protocol::ExecutorReleaseParams = parse(params, method)?;
            respond(
                server
                    .browser
                    .release(p.expected_generation)
                    .await
                    .map_err(internal)?,
            )
        }
        methods::BROWSER_LIST => {
            let p: jeeves_protocol::BrowserListParams = parse(params, method)?;
            respond(server.browser.list(server, &p.session_id).await?)
        }
        methods::BROWSER_START => {
            let p: jeeves_protocol::BrowserStartParams = parse(params, method)?;
            respond(server.browser.start(server, p).await?)
        }
        methods::BROWSER_INPUT => respond(
            server
                .browser
                .human_input(parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::BROWSER_DOWNLOADS => {
            let p: jeeves_protocol::TaskGetParams = parse(params, method)?;
            respond(
                server
                    .browser
                    .downloads(&p.task_id)
                    .await
                    .map_err(internal)?,
            )
        }
        methods::BROWSER_DOWNLOAD => respond(
            server
                .browser
                .decide_download(server, parse(params, method)?)
                .await
                .map_err(internal)?,
        ),
        methods::BROWSER_CONTROL => {
            let p: jeeves_protocol::BrowserControlParams = parse(params, method)?;
            respond(server.browser.control(p).await?)
        }
        methods::BROWSER_FRAME => {
            let p: jeeves_protocol::TaskGetParams = parse(params, method)?;
            respond(server.browser.frame(&p.task_id).await?)
        }
        methods::APPROVAL_NOTICE => {
            respond(crate::approval_push::notice(server, parse(params, method)?).await?)
        }
        methods::APPROVAL_NOTICE_DECISION => {
            respond(crate::approval_push::decision(server, parse(params, method)?).await?)
        }
        methods::APPROVAL_PUSH_KEY => respond(crate::approval_push::public_key(server)?),
        methods::APPROVAL_PUSH_SUBSCRIBE => {
            respond(crate::approval_push::subscribe(server, parse(params, method)?).await?)
        }
        methods::APPROVAL_PENDING => {
            let p: jeeves_protocol::ApprovalPendingParams = parse(params, method)?;
            respond(crate::pending_approvals::list(server, &p.session_id).await?)
        }
        methods::SESSION_PERMISSION_GET => {
            let p: jeeves_protocol::SessionPermissionGetParams = parse(params, method)?;
            respond(server.permission_view(&p.session_id).await?)
        }
        methods::SESSION_PERMISSION_SET => {
            let p: jeeves_protocol::SessionPermissionSetParams = parse(params, method)?;
            respond(server.set_permission(p, sink).await?)
        }

        methods::TURN_OPERATIONS => {
            let params: jeeves_protocol::TurnOperationsParams = parse(params, method)?;
            respond(crate::turn::operations::inspect(server, &params).await?)
        }
        methods::TURN_RECONCILE => {
            let params: jeeves_protocol::TurnReconcileParams = parse(params, method)?;
            respond(crate::turn::operations::reconcile(server, params).await?)
        }
        methods::MEMORY_SEARCH => {
            let p: MemorySearchParams = parse(params, method)?;
            let top_k = p.top_k.unwrap_or(DEFAULT_TOP_K);
            let facts = server
                .facts
                .search(&p.query, top_k)
                .await
                .map_err(internal)?;
            let episodes = server
                .episodes
                .search(&p.query, EPISODE_HITS)
                .await
                .map_err(internal)?;
            respond(MemorySearchResponse {
                facts: facts.into_iter().map(to_fact).collect(),
                episodes: episodes.into_iter().map(to_episode).collect(),
            })
        }

        methods::MEMORY_LIST => {
            let p: MemoryListParams = parse(params, method)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
            let offset = p
                .cursor
                .as_deref()
                .and_then(|c| c.parse::<u32>().ok())
                .unwrap_or(0);
            let rows = server.facts.recent(limit, offset).await.map_err(internal)?;
            let next = (rows.len() as u32 == limit).then(|| (offset + limit).to_string());
            respond(MemoryListResponse {
                data: rows.into_iter().map(to_fact).collect(),
                next_cursor: next,
            })
        }

        methods::MEMORY_LIST_EPISODES => {
            let p: MemoryListEpisodesParams = parse(params, method)?;
            let limit = p.limit.unwrap_or(EPISODE_LIMIT);
            let rows = server.episodes.recent(limit).await.map_err(internal)?;
            respond(MemoryListEpisodesResponse {
                data: rows.into_iter().map(to_episode).collect(),
                next_cursor: None,
            })
        }

        methods::MEMORY_VALIDITY => {
            let p: jeeves_protocol::MemoryValidityParams = parse(params, method)?;
            let changed = server
                .facts
                .set_validity(
                    i64::from(p.fact_id),
                    p.valid_from.as_deref(),
                    p.valid_until.as_deref(),
                )
                .await
                .map_err(internal)?;
            respond(jeeves_protocol::MemoryValidityResponse { changed })
        }
        methods::MEMORY_PROVENANCE => {
            let p: jeeves_protocol::MemoryProvenanceParams = parse(params, method)?;
            let (valid_from, valid_until) = server
                .facts
                .validity(i64::from(p.fact_id))
                .await
                .map_err(internal)?;
            let observations = server
                .facts
                .observations_detailed(i64::from(p.fact_id))
                .await
                .map_err(internal)?;
            let observations = observations
                .into_iter()
                .map(|(context_digest, refs, observed_at, assessment_json)| {
                    Ok(jeeves_protocol::MemoryObservation {
                        context_digest,
                        source_refs: serde_json::from_str(&refs)?,
                        observed_at,
                        assessment_json,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(internal)?;
            respond(jeeves_protocol::MemoryProvenanceResponse {
                fact_id: p.fact_id,
                valid_from,
                valid_until,
                validity_origin: server
                    .facts
                    .validity_origin(i64::from(p.fact_id))
                    .await
                    .map_err(internal)?,
                observations,
            })
        }
        methods::MEMORY_REMEMBER => {
            let p: MemoryRememberParams = parse(params, method)?;
            let source = p
                .source
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "user".to_string());
            let bundle = jeeves_memory::bundle::Bundle::new(&server.settings().home);
            let change = jeeves_memory::store::Change::Remember {
                subject: &p.subject,
                content: &p.content,
                source: &source,
                kind: "fact",
            };
            let row = match jeeves_memory::store::apply(&server.facts, &bundle, change)
                .await
                .map_err(internal)?
            {
                jeeves_memory::store::Outcome::Remembered { row, .. } => row,
                _ => unreachable!("Remember answers Remembered"),
            };
            respond(MemoryRememberResponse { fact: to_fact(row) })
        }

        methods::MEMORY_FORGET => {
            let p: MemoryForgetParams = parse(params, method)?;
            let bundle = jeeves_memory::bundle::Bundle::new(&server.settings().home);
            let change = jeeves_memory::store::Change::ForgetSubject {
                subject: &p.subject,
            };
            let removed = match jeeves_memory::store::apply(&server.facts, &bundle, change)
                .await
                .map_err(internal)?
            {
                jeeves_memory::store::Outcome::Forgot(removed) => removed,
                _ => unreachable!("ForgetSubject answers Forgot"),
            };
            respond(MemoryForgetResponse {
                removed: narrow(removed as i64),
            })
        }

        methods::SESSION_LIST => {
            let p: SessionListParams = parse(params, method)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIMIT) as usize;
            let offset = p
                .cursor
                .as_deref()
                .and_then(|c| c.parse::<usize>().ok())
                .unwrap_or(0);
            let all: Vec<_> = server
                .chat
                .sessions()
                .await
                .map_err(internal)?
                .into_iter()
                .filter(|session| session.id == crate::interaction::MAIN_SESSION)
                .collect();
            let data: Vec<_> = all
                .iter()
                .skip(offset)
                .take(limit)
                .cloned()
                .map(to_session)
                .collect();
            let next = (offset + limit < all.len()).then(|| (offset + limit).to_string());
            respond(SessionListResponse {
                data,
                next_cursor: next,
            })
        }

        methods::SESSION_GET => {
            let p: SessionGetParams = parse(params, method)?;
            let row = server.chat.session(&p.session_id).await.map_err(internal)?;
            match row {
                Some(row) => respond(SessionGetResponse {
                    session: to_session(row),
                }),
                None => Err(no_such_session(&p.session_id)),
            }
        }

        methods::SESSION_UPDATE => {
            let p: SessionUpdateParams = parse(params, method)?;
            crate::interaction::require_main(Some(&p.session_id))?;
            let cwd = match p.cwd.as_deref() {
                None => None,
                Some(text) if text.trim().is_empty() => Some(String::new()),
                Some(text) => checked_cwd(Some(text))?,
            };
            server
                .chat
                .update_session(&p.session_id, p.title.as_deref(), cwd.as_deref())
                .await
                .map_err(internal)?;
            match server.chat.session(&p.session_id).await.map_err(internal)? {
                Some(row) => respond(SessionUpdateResponse {
                    session: to_session(row),
                }),
                None => Err(no_such_session(&p.session_id)),
            }
        }

        methods::TURN_LIST => {
            let p: TurnListParams = parse(params, method)?;
            let turns = server
                .active_turns(p.session_id.as_deref())
                .into_iter()
                .map(|(turn_id, session_id, started_at)| TurnView {
                    turn_id,
                    session_id,
                    started_at,
                })
                .collect();
            respond(TurnListResponse { turns })
        }

        methods::SESSION_MESSAGES => {
            let p: SessionMessagesParams = parse(params, method)?;
            let limit = p.limit.unwrap_or(DEFAULT_LIMIT);
            let before = p.cursor.as_deref().and_then(|c| c.parse::<i64>().ok());
            let task_session = p.session_id.starts_with("task-work:")
                && sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM task_dispatch WHERE session_id=?)",
                )
                .bind(&p.session_id)
                .fetch_one(&server.pool)
                .await
                .map_err(internal)?;
            let _snapshot = if task_session && p.cursor.is_none() {
                Some(server.session_write_lock(&p.session_id).lock_owned().await)
            } else {
                None
            };
            let rows = server
                .chat
                .messages(&p.session_id, before, limit)
                .await
                .map_err(internal)?;

            let next_cursor = (rows.len() as u32 == limit)
                .then(|| rows.last().map(|row| row.id.to_string()))
                .flatten();

            let active_inputs = if task_session && p.cursor.is_none() {
                crate::task::dispatch::active_inputs(server, &p.session_id).await?
            } else {
                None
            };
            let mut data: Vec<_> = rows.into_iter().filter_map(to_message).collect();
            if task_session {
                for message in &mut data {
                    if message.role == MessageRole::User {
                        message.content =
                            crate::interaction::task_tool::visible_instruction(&message.content)
                                .to_owned();
                    }
                }
            }
            respond(SessionMessagesResponse {
                active_inputs,
                data,
                next_cursor,
            })
        }

        methods::CONFIG_READ => {
            let _p: ConfigReadParams = parse(params, method)?;
            respond(ConfigReadResponse {
                config: server.settings().view(),
            })
        }

        methods::CONFIG_WRITE => {
            let p: ConfigWriteParams = parse(params, method)?;
            let view = server
                .apply_config_patch(&p.patch)
                .map_err(|message| ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                })?;
            respond(ConfigWriteResponse { config: view })
        }

        methods::MODEL_PROFILES => {
            if let Some(value) = params {
                if value.as_object().is_some_and(|o| !o.is_empty()) {
                    return Err(ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message: format!("{method} takes no parameters"),
                        data: None,
                    });
                }
            }
            let _: ModelProfilesParams = ModelProfilesParams;
            respond(server.model_profiles())
        }

        methods::MODEL_SAVE => {
            let p: ModelSaveParams = parse(params, method)?;
            match server.model_save(p) {
                Ok(response) => {
                    crate::task::dispatch::kick(server.clone(), sink.clone());
                    respond(response)
                }
                Err(message) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                }),
            }
        }

        methods::MODEL_REMOVE => {
            let p: ModelRemoveParams = parse(params, method)?;
            respond(server.model_remove(p).map_err(internal)?)
        }

        methods::MODEL_ROUTES => {
            if let Some(value) = params {
                if value.as_object().is_some_and(|o| !o.is_empty()) {
                    return Err(ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message: format!("{method} takes no parameters"),
                        data: None,
                    });
                }
            }
            respond(server.model_routes())
        }

        methods::MODEL_ROUTES_SAVE => {
            let p: ModelRoutesSaveParams = parse(params, method)?;
            match server.model_routes_save(p) {
                Ok(response) => respond(response),
                Err(message) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                }),
            }
        }

        methods::USAGE_SUMMARY => {
            let p: UsageSummaryParams = parse(params, method)?;
            respond(server.usage_summary(p).await)
        }

        methods::RESPONSIBILITY_REPORTS => {
            let p: jeeves_protocol::ResponsibilityReportsParams = parse(params, method)?;
            respond(
                crate::responsibility::worker::reports(server, &p.id)
                    .await
                    .map_err(internal)?,
            )
        }
        methods::CONNECTOR_TOOLS | methods::CONNECTOR_TOOLS_SET => {
            let result = if method == methods::CONNECTOR_TOOLS {
                let p: jeeves_protocol::CapabilityProbeParams = parse(params, method)?;
                server.connector_tools(&p.id).await
            } else {
                server.set_connector_tool(parse(params, method)?).await
            };
            respond(result.map_err(|message| ErrorObject {
                code: codes::INVALID_PARAMS,
                message,
                data: None,
            })?)
        }
        methods::CONNECTOR_CATALOG => respond(jeeves_mcp::manifest::catalog()),
        methods::RESPONSIBILITY_PREPARE => respond(crate::responsibility::prepare(
            parse(params, method)?,
            chrono::Utc::now(),
        )?),
        methods::RESPONSIBILITY_REVIEWS => {
            respond(crate::responsibility::reviews::list(server).await?)
        }
        methods::RESPONSIBILITY_DECIDE => {
            respond(crate::responsibility::reviews::decide(server, parse(params, method)?).await?)
        }
        methods::RESPONSIBILITY_SAMPLE => respond(
            crate::responsibility::sample(server, parse(params, method)?, chrono::Utc::now())
                .await?,
        ),
        methods::RESPONSIBILITY_AMEND => respond(
            crate::responsibility::amend(server, parse(params, method)?, chrono::Utc::now())
                .await?,
        ),
        methods::RESPONSIBILITY_REGISTER => respond(
            crate::responsibility::register(server, parse(params, method)?, chrono::Utc::now())
                .await?,
        ),
        methods::RESPONSIBILITY_SET_PAUSED => respond(
            crate::responsibility::set_paused(server, parse(params, method)?, chrono::Utc::now())
                .await?,
        ),
        methods::RESPONSIBILITY_LIST => {
            if params
                .is_some_and(|p| !(p.is_null() || p.as_object().is_some_and(|obj| obj.is_empty())))
            {
                return Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message: format!("{method} takes no parameters"),
                    data: None,
                });
            }
            respond(crate::responsibility::list(server, chrono::Utc::now()).await?)
        }
        methods::SCHEDULE_LIST => {
            if let Some(value) = params {
                if value.as_object().is_some_and(|o| !o.is_empty()) {
                    return Err(ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message: format!("{method} takes no parameters"),
                        data: None,
                    });
                }
            }
            let _: ScheduleListParams = ScheduleListParams;
            respond(server.schedule_list().await)
        }

        methods::SCHEDULE_SAVE => {
            let p: ScheduleSaveParams = parse(params, method)?;
            match server.schedule_save(p).await {
                Ok(response) => respond(response),
                Err(message) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                }),
            }
        }

        methods::SCHEDULE_REMOVE => {
            let p: ScheduleRemoveParams = parse(params, method)?;
            match server.schedule_remove(p).await {
                Ok(response) => respond(response),
                Err(message) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                }),
            }
        }

        methods::APPROVAL_LOG => {
            if let Some(value) = params {
                if value.as_object().is_some_and(|o| !o.is_empty()) {
                    return Err(ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message: format!("{method} takes no parameters"),
                        data: None,
                    });
                }
            }
            let _: ApprovalLogParams = ApprovalLogParams;
            respond(server.approval_log().await)
        }

        methods::MODEL_LIST => {
            let p: ModelListParams = parse(params, method)?;
            let data = server
                .catalog
                .entries()
                .iter()
                .filter(|info| p.provider.as_deref().is_none_or(|want| info.id == want))
                .map(|info| ModelInfo {
                    id: info.model.clone(),
                    provider: info.id.clone(),
                    label: None,
                    flagship: Some(info.model.clone()),
                    fast: info.small_model.clone(),
                })
                .collect();
            respond(ModelListResponse {
                data,
                next_cursor: None,
            })
        }

        methods::SESSION_MAIN => respond(crate::interaction::main_session(server, sink).await?),
        methods::INTERACTION_START => {
            let p: jeeves_protocol::InteractionStartParams = parse(params, method)?;
            crate::interaction::start(server, p, request_id.clone(), sink).await?;
            Ok(Outcome::SelfSent)
        }
        methods::TURN_START => {
            let p: TurnStartParams = parse(params, method)?;
            crate::interaction::submit_main(server, p, request_id.clone(), sink).await?;
            Ok(Outcome::SelfSent)
        }

        methods::TURN_INTERRUPT => {
            let p: TurnInterruptParams = parse(params, method)?;
            let interrupted = server.interrupt_turn(&p.turn_id);
            respond(TurnInterruptResponse { interrupted })
        }

        methods::TURN_STEER => {
            let p: TurnSteerParams = parse(params, method)?;
            let message = p.message.trim();
            if message.is_empty() {
                return Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message: "The follow-up is empty — an empty message should not cost a turn its next model call."
                        .to_string(),
                    data: None,
                });
            }
            let session_id = crate::interaction::require_main(Some(&p.session_id))?.to_string();
            match server.steer(&session_id, message) {
                Ok((turn_id, queued)) => {
                    sink.notification(ServerNotification::TurnSteered(TurnSteeredNotification {
                        turn_id: turn_id.clone(),
                        session_id,
                        message: message.to_string(),
                        ts: chrono::Local::now().to_rfc3339(),
                    }));
                    respond(TurnSteerResponse { turn_id, queued })
                }
                Err(()) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message: format!(
                        "Session {session_id} has no turn running — just call turn/start, no steer needed."
                    ),
                    data: None,
                }),
            }
        }

        methods::TURN_FOLLOW_UP => {
            let p: TurnFollowUpParams = parse(params, method)?;
            let message = p.message.trim();
            if message.is_empty() {
                return Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message:
                        "The message to queue is empty — an empty message should not cost a turn."
                            .to_string(),
                    data: None,
                });
            }
            let session_id = crate::interaction::require_main(Some(&p.session_id))?.to_string();
            match server.follow_up(&session_id, message).await {
                crate::FollowUp::Queued(queued) => {
                    respond(TurnFollowUpResponse { session_id, queued })
                }
                crate::FollowUp::Idle => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message: format!(
                        "Session {session_id} has no turn running — \"after this turn\" has nothing to wait for; just call turn/start."
                    ),
                    data: None,
                }),
                crate::FollowUp::Failed(why) => Err(ErrorObject {
                    code: codes::INTERNAL_ERROR,
                    message: format!("the queue entry was not written to the database: {why}"),
                    data: None,
                }),
            }
        }

        methods::TASK_OPERATIONS => {
            let p: TaskGetParams = parse(params, method)?;
            respond(crate::task::operations::inspect(server, &p.task_id).await?)
        }
        methods::TASK_RECONCILE => {
            let p: jeeves_protocol::TaskReconcileParams = parse(params, method)?;
            respond(crate::task::operations::reconcile(server, sink, p).await?)
        }
        methods::TASK_WAIT => {
            let p: jeeves_protocol::TaskWaitParams = parse(params, method)?;
            respond(crate::task::inputs::declare(server, sink, p).await?)
        }
        methods::TASK_SIGNAL => {
            let p: jeeves_protocol::TaskSignalParams = parse(params, method)?;
            respond(crate::task::inputs::signal(server, p).await?)
        }
        methods::TASK_WAKE => {
            let p: jeeves_protocol::TaskWakeParams = parse(params, method)?;
            respond(crate::task::inputs::wake(server, p).await?)
        }
        methods::TASK_START => {
            let p: jeeves_protocol::TaskStartParams = parse(params, method)?;
            respond(crate::task::start(server, sink, p).await?)
        }
        methods::TASK_CREATE => {
            let mut p: TaskCreateParams = parse(params, method)?;
            let main = crate::interaction::require_main(p.session_id.as_deref())?;
            crate::interaction::main_session(server, sink).await?;
            p.session_id = Some(main.into());
            respond(crate::task::create(server, p).await?)
        }

        methods::TASK_LIST => {
            let p = match params.filter(|v| !v.is_null()) {
                Some(value) => parse(Some(value), method)?,
                None => jeeves_protocol::TaskListParams { cursor: None },
            };
            respond(crate::task::list(server, p.cursor.as_deref()).await?)
        }

        methods::TASK_VIEW => {
            let p: SessionViewParams = parse(params, method)?;
            respond(crate::task::view_of(server, &p.session_id).await?)
        }

        methods::TASK_EVENTS => {
            let p: jeeves_protocol::TaskEventsParams = parse(params, method)?;
            respond(crate::task::events(server, p).await?)
        }

        methods::CAPABILITY_LIST => respond(server.capability_list()),

        methods::CONNECTOR_SETTINGS => {
            respond(server.connector_settings().map_err(|message| ErrorObject {
                code: codes::INVALID_PARAMS,
                message,
                data: None,
            })?)
        }

        methods::CONNECTOR_SETTINGS_SAVE => {
            let params: jeeves_protocol::ConnectorSettingsSaveParams = parse(params, method)?;
            respond(
                server
                    .save_connector_settings(params)
                    .await
                    .map_err(|message| ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message,
                        data: None,
                    })?,
            )
        }

        methods::CONNECTOR_OAUTH_START => {
            let p: jeeves_protocol::ConnectorOAuthStartParams = parse(params, method)?;
            respond(
                server
                    .start_connector_oauth(&p.id)
                    .await
                    .map_err(|message| ErrorObject {
                        code: codes::INVALID_PARAMS,
                        message,
                        data: None,
                    })?,
            )
        }

        methods::CAPABILITY_PROBE => {
            let p: jeeves_protocol::CapabilityProbeParams = parse(params, method)?;
            match server.capability_probe(&p.id) {
                Ok(r) => respond(r),
                Err(message) => Err(ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                }),
            }
        }

        methods::TASK_GET => {
            let p: TaskGetParams = parse(params, method)?;
            respond(crate::task::get(server, &p.task_id).await?)
        }

        methods::TASK_INTEGRATE => {
            respond(crate::task::integration::integrate(server, parse(params, method)?).await?)
        }
        methods::TASK_GIT_CHECKPOINT => {
            let p: jeeves_protocol::TaskGitCheckpointParams = parse(params, method)?;
            respond(crate::task::git_checkpoint::create(server, p).await?)
        }
        methods::TASK_GIT_CHECKPOINT_POLICY => {
            let p: jeeves_protocol::TaskGetParams = parse(params, method)?;
            respond(crate::task::git_checkpoint::policy(server, &p.task_id).await?)
        }
        methods::TASK_GIT_CHECKPOINT_POLICY_SET => {
            let p: jeeves_protocol::TaskGitCheckpointPolicyParams = parse(params, method)?;
            respond(crate::task::git_checkpoint::set_policy(server, p).await?)
        }

        methods::TASK_RUNS => {
            let p: TaskGetParams = parse(params, method)?;
            respond(crate::task::runs(server, &p.task_id).await?)
        }

        methods::TASK_REVIEW_CAPTURE => {
            let p: jeeves_protocol::TaskReviewCaptureParams = parse(params, method)?;
            respond(crate::task::review::capture(server, p).await?)
        }

        methods::TASK_ARTIFACTS => {
            let p: TaskGetParams = parse(params, method)?;
            respond(crate::task::artifacts(server, &p.task_id).await?)
        }

        methods::TASK_SUSPEND => {
            let p: TaskActionParams = parse(params, method)?;
            respond(crate::task::suspend(server, sink, p).await?)
        }

        methods::TASK_RESUME => {
            let p: TaskActionParams = parse(params, method)?;
            respond(crate::task::resume(server, sink, p).await?)
        }

        methods::TASK_CANCEL => {
            let p: TaskActionParams = parse(params, method)?;
            respond(crate::task::cancel(server, sink, p).await?)
        }

        methods::TODO_VIEW => {
            let p: SessionViewParams = parse(params, method)?;
            respond(server.todo_view(&p.session_id))
        }

        methods::APPROVAL_RESPOND => {
            let p: ApprovalRespondParams = parse(params, method)?;
            let actor = p
                .actor
                .as_deref()
                .map(str::trim)
                .filter(|actor| !actor.is_empty())
                .unwrap_or(crate::server::controls::OWNER);
            let accepted = server
                .answer_approval_checked(
                    &p.turn_id,
                    &p.request_id,
                    p.approved,
                    p.remember,
                    p.version,
                    actor,
                )
                .map_err(|message| ErrorObject {
                    code: codes::INVALID_PARAMS,
                    message,
                    data: None,
                })?;
            let answered_by = if accepted {
                None
            } else {
                match server.approval_log.decided(&p.turn_id, &p.request_id).await {
                    Ok(Some((outcome, who)))
                        if matches!(outcome.as_str(), "approved" | "denied") =>
                    {
                        Some(who)
                    }
                    _ => None,
                }
            };
            respond(ApprovalRespondResponse {
                accepted,
                answered_by,
            })
        }

        other => match crate::memory_center::handle(server, other, params).await {
            Some(result) => result.map(Outcome::Value),
            None => Err(ErrorObject {
                code: codes::METHOD_NOT_FOUND,
                message: format!("unknown method '{other}'"),
                data: None,
            }),
        },
    }
}

/// Wrap a `*Response` into a JSON-RPC response.
fn respond<T: serde::Serialize>(value: T) -> Result<Outcome, ErrorObject> {
    let encoded = serde_json::to_value(value).map_err(internal)?;
    Ok(Outcome::Value(encoded))
}

/// Default number of episodic memories returned: fewer than semantic memory, because "recall what
/// happened recently" does not need many items — more would dilute the context.
const EPISODE_HITS: u32 = 3;
/// Default number of rows for listing episodic memory. Different from `EPISODE_HITS`: that one is
/// "retrieval hits stuffed into the prompt", where more dilutes context; this is a list for humans, where fewer would be odd.
const EPISODE_LIMIT: u32 = 20;
const DEFAULT_TOP_K: u32 = 4;
const DEFAULT_LIMIT: u32 = 50;

/// When params is absent, parse as `{}` so all-Option `*Params` can take the Default path.
fn parse<T: DeserializeOwned>(params: Option<&Value>, method: &str) -> Result<T, ErrorObject> {
    let value = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    serde_json::from_value(value).map_err(|e| ErrorObject {
        code: codes::INVALID_PARAMS,
        message: format!("{method} got bad parameters: {e}"),
        data: None,
    })
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ErrorObject {
    ErrorObject {
        code: codes::INTERNAL_ERROR,
        message: e.to_string(),
        data: None,
    }
}

/// A session id that names nothing: a typo, or a row that was never created. The id goes into
/// the message because "which one" is the only thing the caller can act on.
fn no_such_session(id: &str) -> ErrorObject {
    ErrorObject {
        code: codes::INVALID_PARAMS,
        message: format!("No such session: {id}. Call session/main for the main conversation ID."),
        data: None,
    }
}

/// `session/update` accepts only an existing absolute working directory, as `turn/start` does.
fn checked_cwd(cwd: Option<&str>) -> Result<Option<String>, ErrorObject> {
    let Some(text) = cwd.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    let owned = Some(text.to_string());
    turn::resolve_turn_cwd(&owned)?;
    Ok(owned)
}

fn to_fact(row: jeeves_state::FactRow) -> jeeves_protocol::Fact {
    jeeves_protocol::Fact {
        id: narrow(row.id),
        subject: row.subject,
        content: row.content,
        source: row.source,
        kind: row.kind,
        created_at: iso_opt(row.created_at),
    }
}

fn to_episode(row: jeeves_state::EpisodeRow) -> jeeves_protocol::Episode {
    jeeves_protocol::Episode {
        id: narrow(row.id),
        happened_at: row.happened_at,
        summary: row.summary,
        created_at: iso_opt(row.created_at),
    }
}

fn to_session(row: jeeves_state::SessionRow) -> jeeves_protocol::SessionSummary {
    jeeves_protocol::SessionSummary {
        id: row.id,
        title: row.title,
        messages: narrow(row.messages),
        started_at: iso_opt(row.started_at),
        last_at: iso_opt(row.last_at),
        cwd: row.cwd,
    }
}

/// One row of `chat_log` → one protocol message.
///
/// Returns an `Option`: `MessageRole` is a closed set; roles it cannot recognize are **not carried
/// into the protocol** (forcing a value in would be a lie, and would waste the frontend's exhaustive `switch`).
fn to_message(row: jeeves_state::MessageRow) -> Option<jeeves_protocol::Message> {
    let role = match row.role.as_str() {
        "user" => MessageRole::User,
        "assistant" => MessageRole::Assistant,
        _ => return None,
    };
    Some(jeeves_protocol::Message {
        id: narrow(row.id),
        source: match row.source.as_str() {
            "task_decision" | "task_completion" => Some(row.source),
            key if key.starts_with("responsibility:") => Some(row.source),
            _ => None,
        },
        task_id: row.task_id,
        receipt_status: row.receipt_status,
        approval_decisions: crate::completion_delivery::approval_decisions(row.approval_decisions),
        role,
        content: row.content,
        at: iso(row.at),
        meta: row
            .meta
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok()),
    })
}

/// SQLite's `datetime('now')` → the ISO 8601 the protocol wants.
///
/// The db stores `2026-09-19 16:46:49`: UTC, but without a timezone marker and not ISO's `T`
/// separator. When a browser gets it, `new Date("2026-09-19 16:46:49")` is treated as **local time** —
/// in UTC+8 a full 8-hour shift. So `Z` is appended at this outlet, making it an unambiguous instant.
/// **The conversion happens only here**: what is stored stays untouched; everything going out is ISO.
///
/// `datetime('now')` is itself UTC, so appending `Z` is correct, not a guess.
///
/// Unrecognizable input is returned as-is — only a formatting layer; it must not be able to make the whole method error out.
fn iso(at: String) -> String {
    chrono::NaiveDateTime::parse_from_str(&at, "%Y-%m-%d %H:%M:%S")
        .map(|naive| {
            naive
                .and_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        })
        .unwrap_or(at)
}

/// See `iso`. These time columns are nullable in the db.
fn iso_opt(at: Option<String>) -> Option<String> {
    at.map(iso)
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod dispatch_tests;

#[cfg(test)]
mod capabilities_tests {
    use super::negotiate;

    #[test]
    fn unknown_features_are_refused_not_silently_ignored() {
        let (accepted, unsupported) = negotiate(vec![
            "streaming".into(),
            "biometrics".into(),
            "images".into(),
        ]);
        assert_eq!(accepted, vec!["streaming", "images"]);
        assert_eq!(unsupported, vec!["biometrics"]);
    }

    #[test]
    fn an_empty_declaration_is_valid() {
        assert_eq!(negotiate(vec![]), (vec![], vec![]));
    }
}
