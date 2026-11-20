//! The Jeeves desktop shell: a window around the Runtime's own front end.
//!
//! The shell holds no agent logic and no second protocol. It starts `jeeves dashboard` (or
//! attaches to the Runtime named by `JEEVES_RUNTIME_URL`) and loads its address, so the
//! desktop, the browser and the phone all see the same Runtime and the same state.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{mpsc, Mutex},
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// The Runtime child we started (none when attached to a remote one).
#[derive(Default)]
struct Runtime(Mutex<Option<Child>>);

const START_TIMEOUT: Duration = Duration::from_secs(65);
/// `jeeves dashboard` prints `Jeeves cockpit → http://host:port (...)` once it is listening.
const READY_MARK: &str = "Jeeves cockpit → ";

fn binary_path() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("JEEVES_DESKTOP_BINARY") {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("JEEVES_DESKTOP_BINARY 必须是绝对路径".into());
            }
            return Ok(path);
        }
        return Ok(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../jeeves-rs/target/debug/jeeves")
        );
    }
    std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .map(|p| p.join("jeeves"))
        .ok_or_else(|| "找不到 Jeeves 服务".into())
}

fn finder_home(binary: &Path, user_home: &Path) -> PathBuf {
    const LOCAL_RUNTIME: &str =
        "desktop/src-tauri/target/release/bundle/macos/Jeeves.app/Contents/MacOS/jeeves";
    if let Some(root) = binary
        .ancestors()
        .find(|root| binary.strip_prefix(root) == Ok(Path::new(LOCAL_RUNTIME)))
    {
        let data = root.join(".jeeves");
        if data.is_dir() {
            return data;
        }
    }
    user_home.join(".jeeves")
}

/// Start `jeeves dashboard` and wait for the address it announces.
fn start_runtime(app: &tauri::AppHandle) -> Result<(Child, String), String> {
    let binary = binary_path()?;
    let mut command = Command::new(&binary);
    if std::env::var_os("JEEVES_HOME").is_none()
        && std::env::current_dir().map_err(|e| e.to_string())? == Path::new("/")
    {
        let user_home = app
            .path()
            .home_dir()
            .map_err(|e| format!("无法确定用户主目录: {e}"))?;
        command.env("JEEVES_HOME", finder_home(&binary, &user_home));
    }
    let helper = binary.with_file_name("jeeves-window");
    if std::env::var_os("JEEVES_DESKTOP_HELPER").is_none() && helper.is_file() {
        command.env("JEEVES_DESKTOP_HELPER", helper);
    }
    command
        .arg("dashboard")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Ok(dir) = app.path().resource_dir() {
        if std::env::var_os("JEEVES_DASHBOARD_DIR").is_none() && dir.join("dashboard").is_dir() {
            command.env("JEEVES_DASHBOARD_DIR", dir.join("dashboard"));
        }
        let browser = dir.join("browser-runtime");
        if std::env::var_os("JEEVES_BROWSER_RUNTIME").is_none()
            && browser.join("runtime.json").is_file()
        {
            command.env("JEEVES_BROWSER_RUNTIME", browser);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("无法启动 Jeeves 服务 ({}): {e}", binary.display()))?;
    let stdout = child.stdout.take().ok_or("无法读取服务输出")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(rest) = line.strip_prefix(READY_MARK) {
                let _ = tx.send(
                    rest.split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
    });
    match rx.recv_timeout(START_TIMEOUT) {
        Ok(url) => Ok((child, url)),
        Err(reason) => {
            stop_child(&mut child);
            Err(match reason {
                mpsc::RecvTimeoutError::Timeout => "Jeeves 服务在 65 秒内没有就绪",
                mpsc::RecvTimeoutError::Disconnected => {
                    "Jeeves 服务在报告地址前关闭了输出，请查看终端日志"
                }
            }
            .into())
        }
    }
}

fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) } == 0 {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => break,
            }
        }
    }
    if let Err(error) = child.kill().and_then(|_| child.wait()) {
        eprintln!("(jeeves) desktop runtime shutdown failed: {error}");
    }
}

fn window_target(target: Result<String, String>) -> WebviewUrl {
    match target.and_then(|raw| {
        tauri::Url::parse(&raw)
            .ok()
            .filter(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
            .ok_or_else(|| "Jeeves 服务地址必须是有效的 HTTP 或 HTTPS 地址".into())
    }) {
        Ok(url) => WebviewUrl::External(url),
        Err(why) => {
            let mut page = tauri::Url::parse("tauri://localhost/index.html").expect("static URL");
            page.query_pairs_mut().append_pair("error", &why);
            WebviewUrl::App(format!("index.html?{}", page.query().expect("error query")).into())
        }
    }
}

fn open_window(app: &tauri::AppHandle, target: Result<String, String>) -> tauri::Result<()> {
    let url = window_target(target);
    WebviewWindowBuilder::new(app, "main", url)
        .title("Jeeves")
        .inner_size(1180.0, 800.0)
        .min_inner_size(900.0, 640.0)
        .build()?;
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(Runtime::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let target = match std::env::var("JEEVES_RUNTIME_URL") {
                Ok(url) if !url.is_empty() => Ok(url),
                _ => start_runtime(&handle).map(|(child, url)| {
                    *handle.state::<Runtime>().0.lock().expect("runtime lock") = Some(child);
                    url
                }),
            };
            open_window(&handle, target)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Jeeves 桌面端无法启动")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(mut child) = app
                    .state::<Runtime>()
                    .0
                    .lock()
                    .expect("runtime lock")
                    .take()
                {
                    stop_child(&mut child);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finder_uses_checkout_data_or_the_users_home() {
        let checkout =
            std::env::temp_dir().join(format!("jeeves-finder-home-{}", std::process::id()));
        std::fs::create_dir_all(checkout.join(".jeeves")).unwrap();
        let binary = checkout
            .join("desktop/src-tauri/target/release/bundle/macos/Jeeves.app/Contents/MacOS/jeeves");
        let user_home = PathBuf::from("/Users/fixture");
        assert_eq!(finder_home(&binary, &user_home), checkout.join(".jeeves"));
        assert_eq!(
            finder_home(
                std::path::Path::new("/Applications/Jeeves.app/Contents/MacOS/jeeves"),
                &user_home
            ),
            user_home.join(".jeeves")
        );
        std::fs::remove_dir(checkout.join(".jeeves")).unwrap();
        std::fs::remove_dir(checkout).unwrap();
    }

    #[test]
    fn invalid_runtime_addresses_open_the_local_error_page() {
        for raw in ["invalid address", "file:///tmp/fixture", ""] {
            assert!(matches!(window_target(Ok(raw.into())), WebviewUrl::App(_)));
        }
        assert!(matches!(
            window_target(Ok("http://127.0.0.1:7777".into())),
            WebviewUrl::External(_)
        ));
    }
}
