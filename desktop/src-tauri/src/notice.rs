//! A Mac notification when something new waits for the user while Jeeves is not in front.
//!
//! The shell reads the waiting count from the cockpit's title (the same signal as the
//! Dock badge), so the page needs no native permission of its own. Only a rise in the
//! count notifies, and only while Jeeves is not the focused window.
use std::sync::Mutex;

use tauri::{Manager, WebviewWindow};
use tauri_plugin_notification::NotificationExt;

/// The last waiting count the shell saw.
#[derive(Default)]
pub(crate) struct Seen(Mutex<i64>);

pub(crate) fn waiting(window: &WebviewWindow, count: i64) {
    let seen = window.app_handle().state::<Seen>();
    let mut last = seen.0.lock().expect("notice lock");
    let rose = count > *last;
    *last = count;
    drop(last);
    let in_front = window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false);
    if !rose || in_front {
        return;
    }
    let (title, body) = text(count, chinese());
    if let Err(error) = window
        .app_handle()
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        eprintln!("(jeeves) could not post a notification: {error}");
    }
}

fn text(count: i64, chinese: bool) -> (String, String) {
    match (chinese, count) {
        (true, 1) => (
            "Jeeves · 有一件事等你决定".into(),
            "打开 Jeeves 的「工作」处理。".into(),
        ),
        (true, n) => (
            format!("Jeeves · {n} 件事等你决定"),
            "打开 Jeeves 的「工作」处理。".into(),
        ),
        (false, 1) => (
            "Jeeves · One item needs your decision".into(),
            "Open Work in Jeeves to handle it.".into(),
        ),
        (false, n) => (
            format!("Jeeves · {n} items need your decision"),
            "Open Work in Jeeves to handle them.".into(),
        ),
    }
}

#[cfg(target_os = "macos")]
fn chinese() -> bool {
    objc2_foundation::NSLocale::preferredLanguages()
        .firstObject()
        .is_some_and(|lang| lang.to_string().starts_with("zh"))
}

#[cfg(not(target_os = "macos"))]
fn chinese() -> bool {
    std::env::var("LANG").is_ok_and(|lang| lang.starts_with("zh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_counts_what_waits_in_the_systems_language() {
        assert_eq!(text(1, true).0, "Jeeves · 有一件事等你决定");
        assert_eq!(text(3, true).0, "Jeeves · 3 件事等你决定");
        assert_eq!(text(1, false).0, "Jeeves · One item needs your decision");
        assert_eq!(text(2, false).0, "Jeeves · 2 items need your decision");
    }
}
