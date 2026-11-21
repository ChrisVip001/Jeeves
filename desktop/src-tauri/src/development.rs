//! The Jeeves desktop shell: a window around the Runtime's own front end.
//!
//! The shell holds no agent logic and no second protocol. It starts `jeeves dashboard` (or
//! attaches to the service named by `JEEVES_RUNTIME_URL`). A local Runtime can declare its
//! HTTPS gateway as the window's entry point in its data directory.
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

fn gateway_target(config: &str, runtime: &str) -> Result<String, String> {
    let mut lines = config.lines().map(str::trim);
    let gateway = lines.next().and_then(|raw| local_origin(raw, "https"));
    let expected = lines.next().and_then(|raw| local_origin(raw, "http"));
    let (Some(mut gateway), Some(expected), None) = (gateway, expected, lines.next()) else {
        return Err("desktop-gateway 需要两行：本机 HTTPS 网关地址和 HTTP Runtime 地址".into());
    };
    if local_origin(runtime, "http").as_ref() != Some(&expected) {
        return Err(format!(
            "网关配置的 Runtime 是 {expected}，本次启动地址是 {runtime}"
        ));
    }
    gateway.set_path("/app/");
    Ok(gateway.to_string())
}

fn desktop_target(home: &Path, runtime: String) -> Result<String, String> {
    match std::fs::read_to_string(home.join("desktop-gateway")) {
        Ok(config) => gateway_target(&config, &runtime),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(runtime),
        Err(error) => Err(format!("无法读取 desktop-gateway: {error}")),
    }
}

/// Start `jeeves dashboard` and wait for the address it announces.
fn start_runtime(app: &tauri::AppHandle) -> Result<(Child, String, PathBuf), String> {
    let binary = binary_path()?;
    let home = runtime_home(app, &binary)?;
    let mut command = Command::new(&binary);
    command.env("JEEVES_HOME", &home);
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
        Ok(url) => Ok((child, url, home)),
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
    WebviewWindowBuilder::new(app, "main", url)
        .title("Jeeves")
        .inner_size(1180.0, 800.0)
        .min_inner_size(900.0, 640.0)
        .build()?;
    Ok(())
}

pub(super) fn main() {
    tauri::Builder::default()
        .manage(Runtime::default())
        .setup(|app| {
            let handle = app.handle().clone();
            let target = match std::env::var("JEEVES_RUNTIME_URL") {
                Ok(url) if !url.is_empty() => Ok(url),
                _ => start_runtime(&handle).and_then(|(child, url, home)| {
                    *handle.state::<Runtime>().0.lock().expect("runtime lock") = Some(child);
                    desktop_target(&home, url)
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
    fn the_window_uses_the_gateway_configured_for_the_started_runtime() {
        let config = "https://localhost:8743\nhttp://127.0.0.1:7777\n";
        assert_eq!(
            gateway_target(config, "http://127.0.0.1:7777").unwrap(),
            "https://localhost:8743/app/"
        );
        assert!(gateway_target(config, "http://127.0.0.1:7778").is_err());
        assert!(gateway_target(
            "https://example.test\nhttp://127.0.0.1:7777",
            "http://127.0.0.1:7777"
        )
        .is_err());
        assert!(gateway_target(
            "https://localhost:8743/account\nhttp://127.0.0.1:7777",
            "http://127.0.0.1:7777"
        )
        .is_err());
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
            assert!(matches!(window_target(Ok(raw.into())), WebviewUrl::App(_)));
        }
        assert!(matches!(
            window_target(Ok("http://127.0.0.1:7777".into())),
            WebviewUrl::External(_)
        ));
    }
}
