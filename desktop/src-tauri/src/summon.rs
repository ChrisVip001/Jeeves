//! ⌥Space brings Jeeves forward from any app and puts the cursor in the message box;
//! pressing it again while Jeeves is in front hides the window. If another app already
//! owns the shortcut, Jeeves keeps running without one.
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Builder, ShortcutState};

pub(crate) const SHORTCUT: &str = "alt+space";

/// Focuses the composer once the page is in front. Harmless on the local error page.
const FOCUS_COMPOSER: &str = "document.getElementById('input')?.focus()";

pub(crate) fn install(app: &AppHandle) {
    let plugin = match Builder::new().with_shortcut(SHORTCUT) {
        Ok(builder) => builder
            .with_handler(|app, _, event| {
                if event.state == ShortcutState::Pressed {
                    toggle(app);
                }
            })
            .build(),
        Err(error) => {
            eprintln!("(jeeves) {SHORTCUT} is unavailable: {error}");
            return;
        }
    };
    if let Err(error) = app.plugin(plugin) {
        eprintln!("(jeeves) {SHORTCUT} is unavailable: {error}");
    }
}

fn toggle(app: &AppHandle) {
    let Some(main) = app.get_webview_window("main") else {
        return;
    };
    let in_front = main.is_visible().unwrap_or(false)
        && main.is_focused().unwrap_or(false)
        && !main.is_minimized().unwrap_or(false);
    if in_front {
        crate::window_state::remember(&main);
        // Like ⌘H: macOS hands focus back to the app the user came from.
        #[cfg(target_os = "macos")]
        let hidden = app.hide();
        #[cfg(not(target_os = "macos"))]
        let hidden = main.hide();
        if let Err(error) = hidden {
            eprintln!("(jeeves) could not hide the window: {error}");
        }
        return;
    }
    crate::window_state::reveal(app);
    if let Err(error) = main.eval(FOCUS_COMPOSER) {
        eprintln!("(jeeves) could not focus the message box: {error}");
    }
}
