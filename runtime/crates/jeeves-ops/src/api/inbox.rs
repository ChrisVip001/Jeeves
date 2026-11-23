//! The inbox: everything waiting on the owner, in one read-only list.
//!
//! Nothing is stored here. Items are derived from what already exists: tasks that need a human
//! call (`needs_attention`) and drafts Jeeves left in `<home>/outbox/` (briefings, messages it was
//! asked to write but never sends). Approvals keep their own live cards in the chat.

use super::failed;
use crate::AppServer;
use axum::{
    Json, extract::Extension, http::StatusCode, response::IntoResponse, response::Response,
};
use jeeves_protocol::{ErrorObject, TaskListResponse, codes, methods};
use serde::Serialize;
use serde_json::json;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

const MAX_DRAFTS: usize = 30;
const PREVIEW_CHARS: usize = 400;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// `task` (needs your call) or `draft` (waiting in the outbox).
    pub kind: &'static str,
    pub id: String,
    pub title: String,
    pub preview: String,
    pub at: String,
}

/// Drafts in the outbox, newest first (file names start with a timestamp).
pub(crate) fn drafts(home: &Path) -> io::Result<Vec<InboxItem>> {
    let outbox = home.join("outbox");
    let entries = match std::fs::read_dir(&outbox) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !name.starts_with('.') && entry.file_type()?.is_file() {
            names.push(name);
        }
    }
    names.sort_unstable_by(|a, b| b.cmp(a));
    let mut items = Vec::new();
    for name in names {
        let file = match std::fs::File::open(outbox.join(&name)) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        let mut bytes = Vec::new();
        file.take((4 * PREVIEW_CHARS) as u64)
            .read_to_end(&mut bytes)?;
        let text: String = String::from_utf8_lossy(&bytes)
            .chars()
            .take(PREVIEW_CHARS)
            .collect();
        let title = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or(&name)
            .to_string();
        items.push(InboxItem {
            kind: "draft",
            at: name.chars().take(15).collect(),
            preview: text,
            title,
            id: name,
        });
        if items.len() == MAX_DRAFTS {
            break;
        }
    }
    Ok(items)
}

pub async fn inbox(Extension(app): Extension<Arc<AppServer>>) -> Response {
    let mut items: Vec<InboxItem> = match app
        .request::<TaskListResponse>(methods::TASK_LIST, json!({}))
        .await
    {
        Ok(list) => list
            .tasks
            .into_iter()
            .filter(|task| task.status == "needs_attention")
            .map(|task| InboxItem {
                kind: "task",
                id: task.id,
                title: task.title,
                preview: String::new(),
                at: task.created_at,
            })
            .collect(),
        Err(error) => return failed(StatusCode::BAD_GATEWAY, error),
    };
    match drafts(app.home()) {
        Ok(drafts) => items.extend(drafts),
        Err(e) => {
            return failed(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorObject {
                    code: codes::INTERNAL_ERROR,
                    message: format!("Could not read the outbox: {e}"),
                    data: None,
                },
            );
        }
    }
    Json(json!({ "items": items })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafts_come_newest_first_and_dotfiles_are_ignored() {
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join("outbox");
        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join("20261117T080000-brief.txt"), "To: me\n\nold").unwrap();
        std::fs::write(outbox.join("20261118T080000-brief.txt"), "To: me\n\nnew").unwrap();
        std::fs::write(outbox.join(".hidden"), "x").unwrap();
        let items = drafts(home.path()).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "20261118T080000-brief.txt");
        assert_eq!(items[0].title, "To: me");
        assert!(items[0].preview.contains("new"));
    }

    #[test]
    fn draft_titles_and_previews_are_bounded_on_one_long_unicode_line() {
        let home = tempfile::tempdir().unwrap();
        let outbox = home.path().join("outbox");
        std::fs::create_dir(&outbox).unwrap();
        std::fs::write(outbox.join("draft.txt"), "文".repeat(5000)).unwrap();
        let items = drafts(home.path()).unwrap();
        assert_eq!(items[0].title.chars().count(), PREVIEW_CHARS);
        assert_eq!(items[0].preview.chars().count(), PREVIEW_CHARS);
    }

    #[test]
    fn a_missing_outbox_is_an_empty_inbox() {
        assert!(
            drafts(tempfile::tempdir().unwrap().path())
                .unwrap()
                .is_empty()
        );
    }
}
