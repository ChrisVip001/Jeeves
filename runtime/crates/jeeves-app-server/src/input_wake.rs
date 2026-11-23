//! External data releases a durable wait, never a new task or additional permissions.
use crate::{EventSink, Server};
use jeeves_state::external_waits::{ExternalWaits, InputWait, INPUT_WAIT_REASON};

pub(crate) fn arm(server: Server, sink: EventSink, wait: InputWait) {
    tokio::spawn(async move {
        let rows = ExternalWaits::new(server.pool.clone());
        let mut next_connector_poll = tokio::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let current = match rows.waiting(&wait.task_id).await {
                Ok(Some(current))
                    if current.state == "waiting"
                        && current.condition_id == wait.condition_id
                        && current.run_id == wait.run_id
                        && current.turn_id == wait.turn_id =>
                {
                    current
                }
                Ok(_) => return,
                Err(error) => {
                    eprintln!("(jeeves) input wait unavailable: {error}");
                    continue;
                }
            };
            let task = match server.tasks.get(&wait.task_id).await {
                Ok(Some(task)) => task,
                Ok(None) => return,
                Err(error) => {
                    eprintln!("(jeeves) input task unavailable: {error}");
                    continue;
                }
            };
            if task.status != jeeves_state::TaskStatus::Suspended
                || task.suspend_reason.as_deref() != Some(INPUT_WAIT_REASON)
            {
                let _ = rows.cancel(&wait.task_id).await;
                return;
            }
            match rows.expired(&wait.task_id).await {
                Ok(true) => {
                    if review_expired(&server, &sink, &wait).await {
                        return;
                    }
                    continue;
                }
                Ok(false) => (),
                Err(error) => {
                    eprintln!("(jeeves) input deadline unavailable: {error}");
                    continue;
                }
            }
            let condition = match rows.get(&wait.task_id).await {
                Ok(Some(condition)) if condition.event_id.is_some() => condition,
                Ok(Some(condition)) if tokio::time::Instant::now() >= next_connector_poll => {
                    if let Some(kind) =
                        crate::connector_wait::kind(&condition.source, &condition.event_key)
                    {
                        next_connector_poll =
                            tokio::time::Instant::now() + std::time::Duration::from_mins(1);
                        if let Err(_error) =
                            crate::connector_wait::poll(&server, &rows, &condition, kind).await
                        {
                            eprintln!(
                                "(jeeves) task {}: connector input poll failed; wait retained",
                                wait.task_id
                            );
                        }
                    }
                    continue;
                }
                Ok(_) => continue,
                Err(error) => {
                    eprintln!("(jeeves) input data unavailable: {error}");
                    continue;
                }
            };
            if server.resolved().is_none()
                || !server.active_turns(Some(&wait.session_id)).is_empty()
            {
                continue;
            }
            if resume_input(&server, &sink, current, &condition).await {
                return;
            }
        }
    });
}

async fn resume_input(
    server: &Server,
    sink: &EventSink,
    wait: jeeves_state::external_waits::InputWait,
    condition: &jeeves_state::external_waits::InputCondition,
) -> bool {
    let mut row = match server.checkpoints.get(&wait.turn_id).await {
        Ok(Some(row)) if row.status == "waiting_input" => row,
        _ => return false,
    };
    let mut messages: Vec<jeeves_provider::Message> = match serde_json::from_str(&row.messages_json)
    {
        Ok(messages) => messages,
        Err(error) => {
            crate::task::contract::stop_recovery(
                server,
                sink,
                &row,
                &format!("Input checkpoint is invalid: {error}"),
            )
            .await;
            return false;
        }
    };
    if let Err(error) = crate::task::recovery::require_known_messages(&messages) {
        crate::task::contract::stop_recovery(server, sink, &row, &error.message).await;
        return false;
    }
    if condition.condition_id != wait.condition_id
        || condition.task_id != wait.task_id
        || condition.event_id.is_none()
    {
        return false;
    }
    let note = match crate::task::inputs::input_message(condition) {
        Ok(note) => note,
        Err(error) => {
            crate::task::contract::stop_recovery(
                server,
                sink,
                &row,
                &format!("External input is invalid: {error}"),
            )
            .await;
            return false;
        }
    };
    let key = format!("input:{}", condition.condition_id);
    if !crate::wake::register(server, &key, &wait.task_id, "").await {
        return true;
    }
    messages.push(note);
    row.messages_json = match serde_json::to_string(&messages) {
        Ok(json) => json,
        Err(_) => return false,
    };
    let resumed =
        crate::resume::resume_turn(server, sink, row, Some(crate::wake::Wake::Input(wait))).await;
    if resumed {
        crate::wake::settle(server, &key, false).await;
    }
    resumed
}

/// Expiry observations may arrive after a human control transition.
pub(crate) async fn review_expired(server: &Server, sink: &EventSink, wait: &InputWait) -> bool {
    let lock = server.session_write_lock(&format!("task-control:{}", wait.task_id));
    let _guard = lock.lock().await;
    let rows = ExternalWaits::new(server.pool.clone());
    let active = match rows.waiting(&wait.task_id).await {
        Ok(Some(current)) => {
            current.state == "waiting"
                && current.run_id == wait.run_id
                && current.turn_id == wait.turn_id
                && current.condition_id == wait.condition_id
        }
        Ok(None) => false,
        Err(error) => {
            eprintln!("(jeeves) cannot review input expiry: {error}");
            return false;
        }
    };
    if !active {
        return true;
    }
    match rows.expired(&wait.task_id).await {
        Ok(false) => return false,
        Ok(true) => (),
        Err(error) => {
            eprintln!("(jeeves) cannot verify input expiry: {error}");
            return false;
        }
    }
    let checkpoint = match server.checkpoints.get(&wait.turn_id).await {
        Ok(Some(cp))
            if cp.run_id.as_deref() == Some(wait.run_id.as_str())
                && cp.session_id == wait.session_id =>
        {
            cp
        }
        Ok(_) => return true,
        Err(error) => {
            eprintln!("(jeeves) input expiry checkpoint unavailable: {error}");
            return false;
        }
    };
    let reason = "Expected input did not arrive before its deadline; human review is required";
    if let Err(error) = server
        .checkpoints
        .review(&checkpoint.turn_id, checkpoint.run_id.as_deref(), reason)
        .await
    {
        eprintln!("(jeeves) input expiry review could not be committed: {error}");
        return false;
    }
    if let Ok(Some(task)) = server.tasks.get(&wait.task_id).await {
        eprintln!(
            "(jeeves) task {}: input expiry reviewed, status {}",
            wait.task_id, task.status
        );
        crate::task::notify(sink, &task, task.last_stop_reason.as_deref());
    }
    if let Err(error) = rows.cancel(&wait.task_id).await {
        eprintln!("(jeeves) input expiry wait could not be closed: {error}");
        return false;
    }
    true
}
