//! `jeeves` — command-line entry point.
//!
//! Bare `jeeves` runs the terminal conversation (holding the app-server's Server
//! in-process); `jeeves app-server` starts the sole process that owns state.db,
//! reading JSON-RPC from stdin and writing responses to stdout; `jeeves runtime`
//! starts the web cockpit, also an app-server client (it forks one to talk to),
//! just translating the protocol into HTTP + SSE.

mod connector_cmd;
mod home_cmd;
mod mcp_cmd;
mod memory_cmd;
mod repl;
mod service_cmd;
mod skill_cmd;

use std::path::PathBuf;

use anyhow::Result;

const USAGE: &str = "\nJeeves — your personal assistant\n\nUsage:\n    jeeves                terminal chat\n    jeeves schedule acknowledge <name>   release an interrupted scheduled slot (jobs themselves fire inside the Runtime)
    jeeves home export|import <dir>   move this home (backup, another machine, local ⇄ cloud)
    jeeves memory         rebuild the plaintext memory library / backfill the ledger\n    jeeves skill [update] list/update skills (index at <home>/skills/index.json by default)\n    jeeves mcp …          list MCP servers; login runs browser OAuth; serve exposes memory as an MCP server\n    jeeves skill …        skills: list / export --to claude,codex / install <url>\n    jeeves eval [path]    deterministic eval (offline 0/1, must be 100% to pass = release gate)\n    jeeves eval --replay  replay eval: recorded conversations + expected event sequences (reads evals/replay/)\n    jeeves judge [path]   one real model turn + a judge scores it (needs a key; the score never blocks a release)\n    jeeves research [path] research-quality: separate citation/review/overall scores on unseen sources (recorded only)\n    jeeves bench [path]   long-task benchmark: success rate and resume rate (recorded only, never blocks)\n    jeeves service …      install / uninstall / status / print a launchd or systemd user unit (nothing starts without --start)\n    jeeves runtime        start the private Runtime HTTP API\n    jeeves app-server     start the JSON-RPC server (stdin/stdout, newline-delimited JSON)\n    jeeves --version      print the version\n\nEnvironment:\n    JEEVES_HOME           state directory, ./.jeeves by default (state.db, SOUL.md, traces/ live here)\n    JEEVES_PORT           private Runtime port, 7777 by default\n";

/// State directory. Defaults to under the current working directory rather than
/// ~/.jeeves — "your memory is a file you can open" must hold true: whichever
/// project you stand in, memory lives next to that project.
fn home() -> PathBuf {
    std::env::var_os("JEEVES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".jeeves"))
}

/// Why Windows cannot boot Jeeves, if it is Windows. The sandbox has no Windows backend
/// (and per the standing rule, **no sandbox, no execution**), hooks run through `sh`,
/// and Windows has no credential store that reads secrets back — booting would produce
/// a half-working assistant while looking perfectly healthy. Say no instead.
/// Unix-only is a stated boundary (docs/operations.md), not a silent limitation.
fn windows_blocker() -> Option<&'static str> {
    if cfg!(windows) {
        Some(
            "Jeeves does not run natively on Windows:\n  \
             · the only exec sandboxes are macOS seatbelt and Linux bubblewrap, and with the \"no sandbox, no exec\" rule\
             run_command would always refuse on Windows;\n  \
             · hooks run through sh;\n  \
             · the Windows credential store cannot read keys back, so only plaintext secrets.json is left.\n\
             Run on macOS or Linux (WSL2 is fine). The support matrix is in docs/operations.md.",
        )
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(why) = windows_blocker() {
        eprintln!("(jeeves) {why}");
        std::process::exit(2);
    }
    let command = std::env::args().nth(1);
    match command.as_deref() {
        Some("mcp") => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            let args: Vec<String> = std::env::args().skip(2).collect();
            mcp_cmd::run(&settings.home, &args).await
        }
        Some("connector") => {
            let home = home();
            let args: Vec<String> = std::env::args().skip(2).collect();
            connector_cmd::run(&home, &args).await
        }
        Some("skill") => {
            let home = home();
            let args: Vec<String> = std::env::args().skip(2).collect();
            skill_cmd::run(&home, &args).await
        }
        Some("bench") => {
            let mut args: Vec<String> = std::env::args().skip(2).collect();
            if args.is_empty() {
                args.push("evals/long".to_string());
            }
            let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
            let code = jeeves_eval::run_long(&paths, &home()).await?;
            std::process::exit(code);
        }
        Some("research") => {
            let mut args: Vec<String> = std::env::args().skip(2).collect();
            if args.is_empty() {
                args.push("evals/research".to_string());
            }
            let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
            let code = jeeves_eval::run_research(&paths, &home()).await?;
            std::process::exit(code);
        }
        Some("eval") | Some("judge") => {
            let mut args: Vec<String> = std::env::args().skip(2).collect();
            let replay = command.as_deref() == Some("eval")
                && matches!(args.first().map(String::as_str), Some("--replay"));
            if replay {
                args.remove(0);
            }
            if args.is_empty() {
                let default = match (command.as_deref(), replay) {
                    (Some("eval"), true) => "evals/replay",
                    (Some("eval"), false) => "evals/deterministic",
                    _ => "evals/judge",
                };
                args.push(default.to_string());
            }
            let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
            let report_home = home();
            let code = match (command.as_deref(), replay) {
                (Some("eval"), true) => jeeves_eval::run_replay(&paths, &report_home).await?,
                (Some("eval"), false) => {
                    jeeves_eval::run_deterministic(&paths, &report_home).await?
                }
                _ => jeeves_eval::run_judge(&paths, &report_home).await?,
            };
            std::process::exit(code);
        }
        Some("app-server") => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            settings.ensure_home()?;
            let mut server = jeeves_app_server::open(&settings).await?;
            server.attach_memory_worker();
            jeeves_app_server::attach_scheduler(server.clone());
            jeeves_app_server::run_stdio(server).await
        }
        Some("service") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            service_cmd::run_command(&home(), &args)
        }
        Some("home") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            home_cmd::run(&home(), &args).await
        }
        Some("memory") => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            settings.ensure_home()?;
            let args: Vec<String> = std::env::args().skip(2).collect();
            memory_cmd::run(&settings, &args).await
        }
        Some("schedule") => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            settings.ensure_home()?;
            let server = jeeves_app_server::open(&settings).await?;
            let args: Vec<String> = std::env::args().skip(2).collect();
            match args.as_slice() {
                [] => anyhow::bail!(
                    "jeeves schedule no longer runs a separate process: the Runtime (`jeeves app-server` / `jeeves runtime`) fires jobs. Use `jeeves schedule acknowledge <name>` to release an interrupted slot."
                ),
                [action, name] if action == "acknowledge" => {
                    jeeves_app_server::acknowledge_schedule(&server, name).await
                }
                _ => anyhow::bail!("Usage: jeeves schedule [acknowledge <name>]"),
            }
        }
        Some("runtime") => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            jeeves_ops::serve(settings).await
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("--version") | Some("-V") => {
            println!("jeeves {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(other) => {
            anyhow::bail!("Unknown subcommand '{other}'\n\n{USAGE}");
        }
        None => {
            let mut settings = jeeves_config::Settings::from_env();
            settings.home = home();
            settings.ensure_home()?;
            let mut server = jeeves_app_server::open(&settings).await?;
            server.attach_memory_worker();
            repl::run(server).await
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_blocker_matches_the_platform() {
        assert_eq!(super::windows_blocker().is_none(), !cfg!(windows));
    }
}
