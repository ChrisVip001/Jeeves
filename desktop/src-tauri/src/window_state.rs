//! The main window behaves like a Mac app: closing it hides it (the Runtime keeps working
//! and the Dock icon brings it back), and it reopens where the user last left it.
//!
//! The frame is stored in logical points as one line `x y width height` in
//! `<app config dir>/window-frame`. A saved frame that no longer fits on any connected
//! display is ignored, so unplugging a monitor never opens the window off-screen.
use std::path::{Path, PathBuf};

use tauri::{Manager, WebviewWindow, WebviewWindowBuilder, Wry};

/// Smallest frame the cockpit lays out cleanly; matches the builder's minimum size.
pub(crate) const MIN_SIZE: (f64, f64) = (900.0, 640.0);

/// A window frame in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Frame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A display's usable area in logical points.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Frame {
    pub(crate) fn parse(line: &str) -> Option<Self> {
        let numbers: Vec<f64> = line
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        let [x, y, width, height] = numbers[..] else {
            return None;
        };
        let frame = Self {
            x,
            y,
            width,
            height,
        };
        (numbers.iter().all(|n| n.is_finite()) && width >= MIN_SIZE.0 && height >= MIN_SIZE.1)
            .then_some(frame)
    }

    pub(crate) fn line(&self) -> String {
        format!(
            "{} {} {} {}\n",
            self.x.round(),
            self.y.round(),
            self.width.round(),
            self.height.round()
        )
    }

    /// The title bar must land on a display and the window must fit inside it.
    pub(crate) fn fits(&self, areas: &[Area]) -> bool {
        areas.iter().any(|a| {
            self.x >= a.x
                && self.y >= a.y
                && self.x + self.width <= a.x + a.width
                && self.y + self.height <= a.y + a.height
        })
    }
}

fn store(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|dir| dir.join("window-frame"))
}

fn areas(app: &tauri::AppHandle) -> Vec<Area> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let scale = m.scale_factor();
            let rect = m.work_area();
            Area {
                x: f64::from(rect.position.x) / scale,
                y: f64::from(rect.position.y) / scale,
                width: f64::from(rect.size.width) / scale,
                height: f64::from(rect.size.height) / scale,
            }
        })
        .collect()
}

fn read(path: &Path) -> Option<Frame> {
    Frame::parse(&std::fs::read_to_string(path).ok()?)
}

/// Places a new window at the remembered frame, or leaves the builder's default size.
pub(crate) fn place<'a>(
    app: &tauri::AppHandle,
    builder: WebviewWindowBuilder<'a, Wry, tauri::AppHandle>,
) -> WebviewWindowBuilder<'a, Wry, tauri::AppHandle> {
    match store(app).as_deref().and_then(read) {
        Some(frame) if frame.fits(&areas(app)) => builder
            .inner_size(frame.width, frame.height)
            .position(frame.x, frame.y),
        _ => builder.center(),
    }
}

/// Remembers where the window is now. Failing to save only costs the next launch its place.
pub(crate) fn remember(window: &WebviewWindow) {
    let (Ok(position), Ok(size), Ok(scale)) = (
        window.outer_position(),
        window.inner_size(),
        window.scale_factor(),
    ) else {
        eprintln!("(jeeves) could not read the window frame; it will not be remembered");
        return;
    };
    let frame = Frame {
        x: f64::from(position.x) / scale,
        y: f64::from(position.y) / scale,
        width: f64::from(size.width) / scale,
        height: f64::from(size.height) / scale,
    };
    let Some(path) = store(window.app_handle()) else {
        eprintln!("(jeeves) no app config directory; the window frame will not be remembered");
        return;
    };
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, frame.line()));
    if let Err(error) = saved {
        eprintln!(
            "(jeeves) could not remember the window frame at {}: {error}",
            path.display()
        );
    }
}

/// Closing the main window hides it instead of quitting; ⌘Q still quits.
pub(crate) fn on_close(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event
        && window.label() == "main"
        && let Some(main) = window.app_handle().get_webview_window("main")
    {
        remember(&main);
        if cfg!(target_os = "macos") {
            api.prevent_close();
            if let Err(error) = main.hide() {
                eprintln!("(jeeves) could not hide the window: {error}");
            }
        }
    }
}

/// Brings the hidden main window back (Dock icon click or the summon shortcut).
pub(crate) fn reveal(app: &tauri::AppHandle) {
    // Undo an app-level hide (⌘H or the summon shortcut) before showing the window.
    #[cfg(target_os = "macos")]
    if let Err(error) = app.show() {
        eprintln!("(jeeves) could not unhide the app: {error}");
    }
    if let Some(main) = app.get_webview_window("main") {
        let shown = main
            .show()
            .and_then(|()| main.unminimize())
            .and_then(|()| main.set_focus());
        if let Err(error) = shown {
            eprintln!("(jeeves) could not show the window: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Area = Area {
        x: 0.0,
        y: 25.0,
        width: 1512.0,
        height: 920.0,
    };

    #[test]
    fn a_saved_frame_round_trips_and_rejects_junk() {
        let frame = Frame::parse("40 60 1180 800").unwrap();
        assert_eq!(Frame::parse(&frame.line()), Some(frame));
        for junk in [
            "",
            "40 60 1180",
            "40 60 1180 800 9",
            "a b c d",
            "0 0 300 200",
            "0 0 NaN 800",
            "0 0 inf 800",
        ] {
            assert_eq!(Frame::parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn a_frame_left_on_an_unplugged_display_is_not_restored() {
        let on_laptop = Frame::parse("40 60 1180 800").unwrap();
        assert!(on_laptop.fits(&[LAPTOP]));
        let on_external = Frame::parse("1600 60 1180 800").unwrap();
        assert!(!on_external.fits(&[LAPTOP]));
        let external = Area {
            x: 1512.0,
            y: 0.0,
            width: 2560.0,
            height: 1415.0,
        };
        assert!(on_external.fits(&[LAPTOP, external]));
        let too_tall = Frame::parse("40 60 1180 1000").unwrap();
        assert!(!too_tall.fits(&[LAPTOP]));
    }
}
