//! The source-development desktop window opens the same Go gateway as the browser.
//!
//! A local gateway profile may ask this window to start the private Runtime it uses.
use std::{
    fs::OpenOptions,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    time::Duration,
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

use crate::window_state::{self, MIN_SIZE};

/// The Runtime child we started (none when attached to a remote one).
#[derive(Default)]
struct Runtime(Mutex<Option<Child>>);

const START_TIMEOUT: Duration = Duration::from_secs(65);
/// `jeeves runtime` announces the private listener when it is ready.
const READY_MARK: &str = "Jeeves Runtime → ";

fn binary_path() -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        if let Some(path) = std::env::var_os("JEEVES_DESKTOP_BINARY") {
            let path = PathBuf::from(path);
            if !path.is_absolute() {
                return Err("JEEVES_DESKTOP_BINARY 必须是绝对路径".into());
            }
            return Ok(path);
        }
        return Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/jeeves"));
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

fn runtime_home(app: &tauri::AppHandle, binary: &Path) -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("JEEVES_HOME") {
        return Ok(PathBuf::from(home));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    if cwd != Path::new("/") {
        return Ok(cwd.join(".jeeves"));
    }
    let user_home = app
        .path()
        .home_dir()
        .map_err(|e| format!("无法确定用户主目录: {e}"))?;
    Ok(finder_home(binary, &user_home))
}

fn local_origin(raw: &str, scheme: &str) -> Option<tauri::Url> {
    tauri::Url::parse(raw).ok().filter(|url| {
        url.scheme() == scheme
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn local_profile(config: &str) -> Result<tauri::Url, String> {
    let mut lines = config.trim().lines();
    let gateway = lines
        .next()
        .and_then(|raw| local_origin(raw, "https").or_else(|| local_origin(raw, "http")));
    let (Some(gateway), None) = (gateway, lines.next()) else {
        return Err("desktop-gateway 只需填写一行回环 Go 网关地址".into());
    };
    Ok(gateway)
}

fn start_runtime(app: &tauri::AppHandle, binary: &Path, home: &Path) -> Result<Child, String> {
    let log = home.join("jeeves.log");
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let stderr = options
        .open(&log)
        .map_err(|e| format!("无法打开 Runtime 日志 ({}): {e}", log.display()))?;
    let mut command = Command::new(binary);
    command
        .env("JEEVES_HOME", home)
        .env_remove("JEEVES_OPS_TOKEN")
        .env("JEEVES_PORT", "0")
        .env("JEEVES_OPS_HOST", "127.0.0.1");
    let helper = binary.with_file_name("jeeves-window");
    if std::env::var_os("JEEVES_DESKTOP_HELPER").is_none() && helper.is_file() {
        command.env("JEEVES_DESKTOP_HELPER", helper);
    }
    command.arg("runtime").stdout(Stdio::piped()).stderr(stderr);
    if let Ok(dir) = app.path().resource_dir() {
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
            if line.starts_with(READY_MARK) {
                let _ = tx.send(());
            }
        }
    });
    match rx.recv_timeout(START_TIMEOUT) {
        Ok(()) => Ok(child),
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

fn desktop_target(app: &tauri::AppHandle) -> Result<String, String> {
    if let Ok(url) = std::env::var("JEEVES_WEB_GATEWAY_URL") {
        return Ok(url);
    }
    let binary = binary_path()?;
    let home = runtime_home(app, &binary)?;
    let config = match std::fs::read_to_string(home.join("desktop-gateway")) {
        Ok(config) => config,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok("http://127.0.0.1:8743".into());
        }
        Err(error) => return Err(format!("无法读取 desktop-gateway: {error}")),
    };
    let gateway = local_profile(&config)?;
    let child = start_runtime(app, &binary, &home)?;
    *app.state::<Runtime>().0.lock().expect("runtime lock") = Some(child);
    Ok(gateway.to_string())
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
            .filter(|url| {
                url.username().is_empty()
                    && url.password().is_none()
                    && url.has_host()
                    && (url.scheme() == "https"
                        || (url.scheme() == "http"
                            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))))
            })
            .ok_or_else(|| "远程服务需要 HTTPS 地址，同机服务可以使用回环 HTTP".into())
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
    let builder = WebviewWindowBuilder::new(app, "main", url)
        .title("Jeeves")
        .inner_size(1180.0, 800.0)
        .min_inner_size(MIN_SIZE.0, MIN_SIZE.1);
    window_state::place(app, builder).build()?;
    Ok(())
}

pub(super) fn main() {
    tauri::Builder::default()
        .manage(Runtime::default())
        .setup(|app| {
            open_window(app.handle(), desktop_target(app.handle()))?;
            Ok(())
        })
        .on_window_event(window_state::on_close)
        .build(tauri::generate_context!())
        .expect("Jeeves 桌面端无法启动")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => window_state::reveal(app),
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit => {
                if let Some(main) = app.get_webview_window("main") {
                    window_state::remember(&main);
                }
                if !matches!(event, tauri::RunEvent::Exit) {
                    return;
                }
                let child = app
                    .state::<Runtime>()
                    .0
                    .lock()
                    .expect("runtime lock")
                    .take();
                if let Some(mut child) = child {
                    stop_child(&mut child);
                }
            }
            _ => (),
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_only_needs_the_stable_gateway_origin() {
        let gateway = local_profile("https://localhost:8743\n").unwrap();
        assert_eq!(gateway.as_str(), "https://localhost:8743/");
        assert!(local_profile("https://localhost:8743\nhttp://127.0.0.1:7777").is_err());
        assert!(local_profile("https://example.test").is_err());
        assert!(local_profile("https://localhost:8743/account").is_err());
    }

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
        for raw in [
            "invalid address",
            "file:///tmp/fixture",
            "",
            "http://runtime.example.test",
            "https://user:pw@runtime.example.test",
        ] {
            std::assert_matches!(window_target(Ok(raw.into())), WebviewUrl::App(_));
        }
        std::assert_matches!(
            window_target(Ok("http://127.0.0.1:7777".into())),
            WebviewUrl::External(_)
        );
    }
}
