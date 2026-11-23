//! `jeeves service install|uninstall|status|print` — keep the Runtime up across logouts and reboots.
//!
//! The supervisor is the operating system's (launchd on macOS, a systemd user unit on Linux), never
//! a second one inside the binary. This module only renders the unit file, writes or removes it,
//! and reports; it starts nothing unless the user passes `--start`. No secret is ever written into
//! a unit: `JEEVES_OPS_TOKEN` and provider keys stay in the environment or the keychain.

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

const LABEL: &str = "ai.jeeves.runtime";
const UNIT: &str = "jeeves.service";

/// Everything a unit needs; all paths absolute.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub binary: PathBuf,
    pub home: PathBuf,
    pub port: u16,
    pub log: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Launchd,
    Systemd,
}

pub fn kind_for(os: &str) -> Result<Kind> {
    match os {
        "macos" => Ok(Kind::Launchd),
        "linux" => Ok(Kind::Systemd),
        other => bail!(
            "no service template for {other}; run `jeeves runtime` under your own supervisor "
        ),
    }
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Unit files are line-oriented: a path with a newline or a double quote cannot be quoted safely.
fn check_path(path: &Path) -> Result<&str> {
    let text = path
        .to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
    if text.contains(['\n', '\r', '"']) {
        bail!("{text:?} has a newline or quote; use a plainer path");
    }
    Ok(text)
}

pub fn render_launchd(spec: &Spec) -> Result<String> {
    let binary = xml(check_path(&spec.binary)?);
    let home = xml(check_path(&spec.home)?);
    let log = xml(check_path(&spec.log)?);
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{binary}</string><string>runtime</string></array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>JEEVES_HOME</key><string>{home}</string>
    <key>JEEVES_PORT</key><string>{port}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
        port = spec.port
    ))
}

pub fn render_systemd(spec: &Spec) -> Result<String> {
    let binary = check_path(&spec.binary)?;
    let home = check_path(&spec.home)?;
    let log = check_path(&spec.log)?;
    Ok(format!(
        "[Unit]\nDescription=Jeeves personal assistant (private resident Runtime)\nAfter=network-online.target\nWants=network-online.target\n\n\
         [Service]\nExecStart=\"{binary}\" runtime\nEnvironment=\"JEEVES_HOME={home}\"\nEnvironment=\"JEEVES_PORT={port}\"\n\
         Restart=on-failure\nRestartSec=5\nTimeoutStopSec=30\nStandardOutput=append:{log}\nStandardError=append:{log}\n\n\
         [Install]\nWantedBy=default.target\n",
        port = spec.port
    ))
}

fn render(kind: Kind, spec: &Spec) -> Result<String> {
    match kind {
        Kind::Launchd => render_launchd(spec),
        Kind::Systemd => render_systemd(spec),
    }
}

fn unit_path(kind: Kind, user_home: &Path) -> PathBuf {
    match kind {
        Kind::Launchd => user_home
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")),
        Kind::Systemd => user_home.join(".config/systemd/user").join(UNIT),
    }
}

/// The commands that load and unload the unit, as a human would type them.
fn start_command(kind: Kind, unit: &Path, uid: &str) -> Vec<String> {
    match kind {
        Kind::Launchd => vec![
            "launchctl".into(),
            "bootstrap".into(),
            format!("gui/{uid}"),
            unit.display().to_string(),
        ],
        Kind::Systemd => vec![
            "systemctl".into(),
            "--user".into(),
            "enable".into(),
            "--now".into(),
            UNIT.into(),
        ],
    }
}

fn stop_command(kind: Kind, unit: &Path, uid: &str) -> Vec<String> {
    match kind {
        Kind::Launchd => vec![
            "launchctl".into(),
            "bootout".into(),
            format!("gui/{uid}"),
            unit.display().to_string(),
        ],
        Kind::Systemd => vec![
            "systemctl".into(),
            "--user".into(),
            "disable".into(),
            "--now".into(),
            UNIT.into(),
        ],
    }
}

fn service_uid(kind: Kind) -> Result<String> {
    if kind == Kind::Systemd {
        return Ok(String::new());
    }
    let out = Command::new("id")
        .arg("-u")
        .output()
        .context("could not determine the launchd user")?;
    if !out.status.success() {
        bail!("id -u failed: {}", out.status);
    }
    let uid: u32 = std::str::from_utf8(&out.stdout)?
        .trim()
        .parse()
        .context("id -u returned an invalid user ID")?;
    Ok(uid.to_string())
}

fn run(command: &[String]) -> Result<()> {
    eprintln!("(jeeves) running: {}", command.join(" "));
    let status = Command::new(&command[0])
        .args(&command[1..])
        .status()
        .with_context(|| format!("could not run {}", command[0]))?;
    if !status.success() {
        bail!("{} exited with {status}", command.join(" "));
    }
    Ok(())
}

fn absolute(path: PathBuf) -> Result<PathBuf> {
    std::path::absolute(&path).with_context(|| format!("cannot resolve {}", path.display()))
}

fn answering(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
}

fn port_from_env() -> u16 {
    std::env::var("JEEVES_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(7777)
}

pub fn run_command(home: &Path, args: &[String]) -> Result<()> {
    let kind = kind_for(std::env::consts::OS)?;
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let home = absolute(home.to_path_buf())?;
    let spec = Spec {
        binary: std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .context("cannot find this binary's path")?,
        log: home.join("jeeves.log"),
        home,
        port: port_from_env(),
    };
    let unit = unit_path(kind, &user_home);
    let sub = args.first().map(String::as_str);
    let start = args.iter().any(|arg| arg == "--start");
    match sub {
        Some("print") => {
            print!("{}", render(kind, &spec)?);
            Ok(())
        }
        Some("install") => {
            let command = start_command(kind, &unit, &service_uid(kind)?);
            let text = render(kind, &spec)?;
            if let Some(dir) = unit.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::create_dir_all(&spec.home)?;
            std::fs::write(&unit, text)?;
            eprintln!("(jeeves) wrote {}", unit.display());
            if start {
                run(&command)
            } else {
                eprintln!(
                    "(jeeves) nothing was started. To start it now and at every login:\n    {}",
                    command.join(" ")
                );
                Ok(())
            }
        }
        Some("uninstall") => {
            if !unit.exists() {
                eprintln!("(jeeves) no unit at {}; nothing to remove", unit.display());
                return Ok(());
            }
            let command = stop_command(kind, &unit, &service_uid(kind)?);
            if let Err(error) = run(&command) {
                eprintln!("(jeeves) could not unload the unit ({error}); removing the file anyway");
            }
            std::fs::remove_file(&unit)?;
            eprintln!(
                "(jeeves) removed {}; state in {} is untouched",
                unit.display(),
                spec.home.display()
            );
            Ok(())
        }
        Some("status") => {
            println!(
                "unit: {} ({})",
                unit.display(),
                if unit.exists() {
                    "installed"
                } else {
                    "not installed"
                }
            );
            println!(
                "port {}: {}",
                spec.port,
                if answering(spec.port) {
                    "answering"
                } else {
                    "not answering"
                }
            );
            Ok(())
        }
        _ => bail!("Usage: jeeves service install [--start] | uninstall | status | print"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> Spec {
        Spec {
            binary: "/opt/jeeves/bin/jeeves".into(),
            home: "/Users/a b/.jeeves".into(),
            port: 7777,
            log: "/Users/a b/.jeeves/jeeves.log".into(),
        }
    }

    #[test]
    fn launchd_restarts_on_failure_with_a_backoff_and_explicit_home() {
        let text = render_launchd(&spec()).unwrap();
        assert!(text.contains("<string>/opt/jeeves/bin/jeeves</string><string>runtime</string>"));
        assert!(text.contains("<key>JEEVES_HOME</key><string>/Users/a b/.jeeves</string>"));
        assert!(text.contains("<key>JEEVES_PORT</key><string>7777</string>"));
        assert!(text.contains("<key>SuccessfulExit</key><false/>"));
        assert!(text.contains("<key>ThrottleInterval</key><integer>10</integer>"));
        assert!(text.contains("<key>RunAtLoad</key><true/>"));
    }

    #[test]
    fn systemd_restarts_on_failure_and_quotes_paths_with_spaces() {
        let text = render_systemd(&spec()).unwrap();
        assert!(text.contains("ExecStart=\"/opt/jeeves/bin/jeeves\" runtime"));
        assert!(text.contains("Environment=\"JEEVES_HOME=/Users/a b/.jeeves\""));
        assert!(text.contains("Restart=on-failure"));
        assert!(text.contains("RestartSec=5"));
        assert!(text.contains("WantedBy=default.target"));
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        let mut odd = spec();
        odd.home = "/tmp/a&b<c>".into();
        let text = render_launchd(&odd).unwrap();
        assert!(text.contains("/tmp/a&amp;b&lt;c&gt;"));
    }

    #[test]
    fn a_path_that_cannot_be_quoted_is_refused() {
        let mut odd = spec();
        odd.home = "/tmp/a\"b".into();
        assert!(render_systemd(&odd).is_err());
        odd.home = "/tmp/a\nb".into();
        assert!(render_launchd(&odd).is_err());
    }

    #[test]
    fn no_secret_ever_reaches_a_unit() {
        for text in [
            render_launchd(&spec()).unwrap(),
            render_systemd(&spec()).unwrap(),
        ] {
            for word in ["TOKEN", "API_KEY", "SECRET", "PASSWORD"] {
                assert!(!text.contains(word), "{word} leaked into a unit");
            }
        }
    }

    #[test]
    fn units_land_where_each_supervisor_looks() {
        let user = Path::new("/Users/me");
        assert_eq!(
            unit_path(Kind::Launchd, user),
            Path::new("/Users/me/Library/LaunchAgents/ai.jeeves.runtime.plist")
        );
        assert_eq!(
            unit_path(Kind::Systemd, user),
            Path::new("/Users/me/.config/systemd/user/jeeves.service")
        );
    }

    #[test]
    fn start_and_stop_are_the_native_commands() {
        let unit = Path::new("/u/x.plist");
        assert_eq!(
            start_command(Kind::Launchd, unit, "501").join(" "),
            "launchctl bootstrap gui/501 /u/x.plist"
        );
        assert_eq!(
            stop_command(Kind::Systemd, unit, "1000").join(" "),
            "systemctl --user disable --now jeeves.service"
        );
    }

    #[test]
    fn other_platforms_are_told_what_to_do_instead() {
        assert_eq!(kind_for("macos").unwrap(), Kind::Launchd);
        assert_eq!(kind_for("linux").unwrap(), Kind::Systemd);
        assert!(
            kind_for("windows")
                .unwrap_err()
                .to_string()
                .contains("supervisor")
        );
    }
}
