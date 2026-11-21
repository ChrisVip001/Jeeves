use crate::{EventSink, Server};
use jeeves_loop::Steering;
use jeeves_protocol::{codes, ErrorObject};
use std::sync::{Arc, Mutex};

/// Positioning info for one turn: session, turn, event outlet, cancellation token, working directory.
/// The same reason as Injections for bundling into a struct: they **always change together**,
/// and clippy stops at the 8th parameter.
#[derive(Clone)]
pub(crate) struct TurnFrame<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) turn_id: &'a str,
    pub(crate) sink: &'a EventSink,
    pub(crate) interaction_tool: Option<&'a jeeves_tools::Tool>,
    pub(crate) interrupt: Option<Arc<jeeves_loop::Interrupt>>,
    pub(crate) worktree: Option<std::path::PathBuf>,
    pub(crate) turn_cwd: Option<std::path::PathBuf>,
    /// Resume from a checkpoint instead of assembling history (crash recovery).
    pub(crate) resume: Option<jeeves_loop::Resume>,
    /// The checkpoint outlet: every tool-results boundary lands here (None = no checkpointing).
    pub(crate) checkpoint: Option<jeeves_loop::CheckpointSink>,
    /// Additions accepted while this turn runs (`turn/steer`). Same channel for every round of
    /// the turn, including stop-hook continuations — a message added during round 2 is still
    /// "about this turn".
    pub(crate) steering: Option<Arc<Mutex<Steering>>>,
}

/// The working directory the session remembers (migration 0018). `None` = it never chose one, or
/// it chose one that has since disappeared — a vanished directory must not silently become "the
/// process cwd": that would turn a missing workspace into writes landing somewhere unexpected.
pub(crate) async fn session_cwd(server: &Server, session_id: &str) -> Option<std::path::PathBuf> {
    let row = server.chat.session(session_id).await.ok().flatten()?;
    let cwd = row.cwd?;
    let path = std::path::PathBuf::from(&cwd);
    if !path.is_dir() {
        eprintln!("(jeeves) the working directory {cwd} recorded for session {session_id} is gone; this turn has no writable scope");
        return None;
    }
    Some(path)
}

/// Admission check for this turn's working directory (fail-closed): when given, it must be an existing
/// absolute path — silent fallback turns "I thought I was working in dir A" into the hardest bug to trace.
/// An empty string counts as not given.
pub(crate) fn resolve_turn_cwd(
    cwd: &Option<String>,
) -> Result<Option<std::path::PathBuf>, ErrorObject> {
    let Some(text) = cwd.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let path = std::path::PathBuf::from(text);
    if !path.is_absolute() {
        return Err(ErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!("cwd '{text}' must be an absolute path"),
            data: None,
        });
    }
    if !path.is_dir() {
        return Err(ErrorObject {
            code: codes::INVALID_PARAMS,
            message: format!("cwd '{text}' does not exist or is not a directory"),
            data: None,
        });
    }
    Ok(Some(path))
}

/// The longest a pre-loop step (local retrieval or compaction) may take before Jeeves
/// proceeds without it. Cancellation is the real protection — this only keeps a hung
/// provider from holding the turn hostage.
pub(super) const PRELOOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Race a pre-loop step against the user's stop.
///
/// Local retrieval and history compaction run before the loop's cancellation check. `None` means
/// the turn was stopped, and the caller proceeds without that step instead of blocking —
/// the loop sees the same cancellation on its first iteration and answers honestly.
pub(crate) async fn interruptible<T>(
    interrupt: Option<&Arc<jeeves_loop::Interrupt>>,
    what: &str,
    future: impl std::future::Future<Output = T>,
) -> Option<T> {
    match interrupt {
        Some(cancel) => tokio::select! {
            _ = cancel.wait() => {
                eprintln!("(jeeves) interrupted: no longer waiting for {what}");
                None
            }
            value = future => Some(value),
        },
        None => Some(future.await),
    }
}

/// The longest slice of the message used as the search query. FTS5 turns every word into an
/// OR term, so an unbounded paste would build an unbounded `MATCH` expression.
const QUERY_CHARS: usize = 300;

/// Memory lookup for this turn: plain keyword search over the user's own message.
///
/// There is no model call here any more. The old retrieval gate spent a small-model round trip
/// per turn to decide whether to search at all; the search itself is a local FTS5 query
/// (microseconds), so it simply runs every turn and the top hits ride in the prompt tail. When
/// the model needs more, it asks with the memory tools.
pub(super) async fn retrieve_memory(
    server: &Server,
    message: &str,
    settings: &jeeves_config::Settings,
    frame: &TurnFrame<'_>,
) -> String {
    if frame.resume.is_some() {
        return String::new();
    }
    let query: String = message.chars().take(QUERY_CHARS).collect();
    match interruptible(
        frame.interrupt.as_ref(),
        "memory retrieval",
        tokio::time::timeout(
            PRELOOP_TIMEOUT,
            jeeves_memory::retrieve_context(
                &server.facts,
                &server.episodes,
                &query,
                settings.retrieval_top_k.max(1) as u32,
            ),
        ),
    )
    .await
    {
        Some(Ok(Ok(text))) => text,
        Some(Ok(Err(error))) => {
            eprintln!("(jeeves) memory lookup unavailable: {error}");
            String::new()
        }
        Some(Err(_)) => {
            eprintln!("(jeeves) memory lookup timed out");
            String::new()
        }
        None => String::new(),
    }
}
