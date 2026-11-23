//! The machine commands of `viv`: saved machines, the server and its bridge, and running a prompt on a machine.
//! All of it is a thin layer over `vivianne_remote`; the rules live there.

use std::io::{BufRead as _, Write as _};
use std::time::Duration;

use vivianne_remote::bridge;
use vivianne_remote::catalog::{Catalog, SavedMachine};
use vivianne_remote::install::{self, Confirmed};
use vivianne_remote::protocol::{
    AgentState, ClientMessage, ServerMessage, WireEvent, read_frame, write_frame,
};
use vivianne_remote::socket;
use vivianne_remote::ssh::LOOPBACK_TARGET;

fn json_of(machine: &SavedMachine) -> serde_json::Value {
    serde_json::json!({
        "id": machine.id,
        "label": machine.label,
        "target": machine.target,
        "session": machine.session,
        "enabled": machine.enabled,
    })
}

fn need<'a>(args: &'a [String], at: usize, what: &str) -> Result<&'a str, String> {
    args.get(at)
        .map(String::as_str)
        .ok_or_else(|| format!("missing {what}"))
}

/// The value after `--flag` in `args`, removing both.
fn take_flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    let value = args.get(at + 1).cloned()?;
    args.drain(at..=at + 1);
    Some(value)
}

fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    args.iter()
        .position(|a| a == flag)
        .map(|at| args.remove(at))
        .is_some()
}

/// `viv machine <add|list|status|reconnect|rename|enable|disable|remove|install> ...`
pub async fn machine(mut args: Vec<String>) -> Result<(), String> {
    let json = take_switch(&mut args, "--json");
    let sub = need(&args, 0, "a machine command (add, list, status, reconnect, rename, enable, disable, remove, install)")?.to_owned();
    let mut catalog = Catalog::load()?;
    match sub.as_str() {
        "add" => {
            let mut rest = args[1..].to_vec();
            let label = take_flag(&mut rest, "--label");
            let session = take_flag(&mut rest, "--remote-session");
            let target = need(
                &rest,
                0,
                "a target (an ssh alias, user@host, or ssh://user@host:port)",
            )?;
            let added = catalog
                .add(target, label.as_deref(), session.as_deref())?
                .clone();
            catalog.save()?;
            if json {
                println!("{}", json_of(&added));
            } else {
                println!(
                    "Saved {} ({}). Run `viv machine status` to see it connect.",
                    added.label, added.target
                );
            }
        }
        "list" => {
            if json {
                println!(
                    "{}",
                    serde_json::Value::Array(catalog.ssh.iter().map(json_of).collect())
                );
            } else if catalog.ssh.is_empty() {
                println!("No machines saved. Add one with `viv machine add <target>`.");
            } else {
                for m in &catalog.ssh {
                    println!(
                        "{}  {}  {}  session {}{}",
                        m.id,
                        m.label,
                        m.target,
                        m.session,
                        if m.enabled { "" } else { "  (off)" }
                    );
                }
            }
        }
        "status" => status(&catalog, json).await?,
        "reconnect" => reconnect(&catalog, need(&args, 1, "a machine label or id")?).await?,
        "rename" => {
            catalog.rename(need(&args, 1, "a machine")?, need(&args, 2, "a new label")?)?;
            catalog.save()?;
        }
        "enable" | "disable" => {
            catalog.set_enabled(need(&args, 1, "a machine")?, sub == "enable")?;
            catalog.save()?;
        }
        "remove" => {
            let gone = catalog.remove(need(&args, 1, "a machine")?)?;
            catalog.save()?;
            println!("Removed {}. What runs on it keeps running.", gone.label);
        }
        "install" => {
            let yes = take_switch(&mut args, "--yes");
            install_on(&catalog, need(&args, 1, "a machine")?, yes).await?;
        }
        other => return Err(format!("`{other}` is not a machine command")),
    }
    Ok(())
}

fn find<'a>(catalog: &'a Catalog, name: &str) -> Result<&'a SavedMachine, String> {
    catalog
        .find(name)
        .ok_or_else(|| format!("No saved machine is called {name}."))
}

/// Connects to every enabled machine once and says what happened.
async fn status(catalog: &Catalog, json: bool) -> Result<(), String> {
    let mut rows = Vec::new();
    for m in &catalog.ssh {
        let outcome = if m.enabled {
            match tokio::time::timeout(Duration::from_secs(30), bridge::open(m, true)).await {
                Ok(Ok(conn)) => Ok(conn.negotiated.server_version.clone()),
                Ok(Err(why)) => Err(why),
                Err(_) => Err("no answer in 30 seconds".to_owned()),
            }
        } else {
            Err("off".to_owned())
        };
        let (state, detail) = match &outcome {
            Ok(v) => ("online", format!("server {v}")),
            Err(why) if why == "off" => ("disabled", String::new()),
            Err(why) => (
                if vivianne_remote::ssh::classify(why).needs_attention() {
                    "attention"
                } else {
                    "offline"
                },
                why.clone(),
            ),
        };
        rows.push((m, state, detail));
    }
    if json {
        let values: Vec<_> = rows
            .iter()
            .map(|(m, s, d)| {
                let mut v = json_of(m);
                v["status"] = (*s).into();
                v["detail"] = d.clone().into();
                v
            })
            .collect();
        println!("{}", serde_json::Value::Array(values));
    } else {
        for (m, s, d) in rows {
            println!(
                "{:<10} {}  {}{}",
                s,
                m.label,
                m.target,
                if d.is_empty() {
                    String::new()
                } else {
                    format!("  ({d})")
                }
            );
        }
    }
    Ok(())
}

/// Native sign-in recovery: runs plain `ssh` in this terminal, so a password or a host-key question is answered by
/// the person, where OpenSSH can ask. Background connections never ask.
async fn reconnect(catalog: &Catalog, name: &str) -> Result<(), String> {
    let m = find(catalog, name)?;
    if m.target == LOOPBACK_TARGET {
        return Err("the loopback machine is this computer".to_owned());
    }
    println!(
        "Connecting to {} with ssh. Answer any question it asks; it will end by itself.",
        m.target
    );
    let status = tokio::process::Command::new(vivianne_remote::ssh::program())
        .args([
            "-o",
            "BatchMode=no",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "NumberOfPasswordPrompts=3",
            "-T",
            &m.target,
            "exit",
        ])
        .env_remove("SSH_ASKPASS")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .status()
        .await
        .map_err(|e| format!("could not run ssh: {e}"))?;
    if status.success() {
        println!("Signed in. The app reconnects by itself.");
        Ok(())
    } else {
        Err(format!("ssh ended with {status}"))
    }
}

/// Asks, then installs `viv` on the machine.
async fn install_on(catalog: &Catalog, name: &str, yes: bool) -> Result<(), String> {
    let m = find(catalog, name)?;
    let platform = install::detect_platform(m).await?;
    let found = install::find_installed(m).await?;
    if !found.is_empty() {
        println!("viv is already on {}: {}", m.label, found.join(", "));
        return Ok(());
    }
    let source = install::choose_source(&platform)?;
    let (bytes, expected_sha) = match &source {
        install::Source::Override(p) | install::Source::ThisProgram(p) => {
            let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
            let sha = install::sha256_hex(&bytes);
            (bytes, sha)
        }
        install::Source::Release { url } => install::download_release(url).await?,
    };
    println!("{}", install::confirmation_question(m, &platform));
    if !yes {
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        if !matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "" | "y" | "yes"
        ) {
            return Err("not installed".to_owned());
        }
    }
    let sha = install::sha256_hex(&bytes);
    println!(
        "{}",
        install::install(m, &bytes, &sha, Confirmed::after_asking())
            .await?
            .trim()
    );
    Ok(())
}

/// `viv [--session S] server <run|start|stop|status>`
pub async fn server(session: &str, args: &[String]) -> Result<(), String> {
    match need(args, 0, "run, start, stop or status")? {
        "run" => vivianne_remote::server::run(session).await,
        "start" => {
            bridge::ensure_server(session).await?;
            println!("The server for session {session} is running.");
            Ok(())
        }
        "status" => {
            let up = socket::is_listening(session).await;
            println!(
                "session {session}: {}",
                if up { "running" } else { "not running" }
            );
            Ok(())
        }
        "sessions" => {
            let root = vivianne_remote::socket::session_dir("x")
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let mut names: Vec<String> = std::fs::read_dir(&root)
                .map(|d| {
                    d.filter_map(Result::ok)
                        .filter(|e| e.path().is_dir())
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect()
                })
                .unwrap_or_default();
            // The default session is always listed, even before it has a folder.
            if !names
                .iter()
                .any(|n| n == vivianne_remote::catalog::DEFAULT_SESSION)
            {
                names.push(vivianne_remote::catalog::DEFAULT_SESSION.to_owned());
            }
            names.sort();
            for name in names {
                let up = socket::is_listening(&name).await;
                println!("{name}  {}", if up { "running" } else { "stopped" });
            }
            Ok(())
        }
        "delete" => {
            let name = need(args, 1, "a session name")?;
            vivianne_remote::catalog::validate_session(name)?;
            if socket::is_listening(name).await {
                return Err(format!(
                    "session {name} is running; stop it first (`viv --session {name} server stop`)"
                ));
            }
            let dir = vivianne_remote::socket::session_dir(name);
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
            }
            println!("session {name}: deleted");
            Ok(())
        }
        "stop" => {
            if !socket::is_listening(session).await {
                println!("session {session}: not running");
                return Ok(());
            }
            let mut stream = socket::connect(session).await.map_err(|e| e.to_string())?;
            let hello = ClientMessage::Hello(vivianne_remote::protocol::client_hello(
                vivianne_remote::CAP_WANTED,
            ));
            write_frame(&mut stream, &hello)
                .await
                .map_err(|e| e.to_string())?;
            let _: ServerMessage = read_frame(&mut stream).await.map_err(|e| e.to_string())?;
            write_frame(&mut stream, &ClientMessage::StopServer)
                .await
                .map_err(|e| e.to_string())?;
            println!("session {session}: stopping");
            Ok(())
        }
        other => Err(format!("`{other}` is not a server command")),
    }
}

/// `viv remote-client-bridge [--check] [--idle-timeout-v1]`, run on the remote by `ssh`.
pub async fn bridge_host(session: &str, args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a == "--check") {
        println!("viv-api-bridge-v1");
        return Ok(());
    }
    bridge::run_host(session, args.iter().any(|a| a == "--idle-timeout-v1")).await
}

/// `viv --machine <label> [--auto|--plan] [--workspace DIR] <prompt>`: runs the prompt on that machine and prints
/// what happens. The model key for the run is sent for this connection only and never stored there.
pub async fn run_on_machine(label: &str, mut args: Vec<String>) -> Result<(), String> {
    let mode = if take_switch(&mut args, "--auto") {
        "auto"
    } else if take_switch(&mut args, "--plan") {
        "plan"
    } else {
        "ask"
    };
    let workspace =
        take_flag(&mut args, "--workspace").ok_or("pass --workspace <folder on that machine>")?;
    let prompt = args.join(" ");
    if prompt.trim().is_empty() {
        return Err("no prompt".to_owned());
    }
    let catalog = Catalog::load()?;
    let machine = find(&catalog, label)?.clone();
    let mut conn = bridge::open(&machine, true).await?;
    let selection = vivianne_providers::env::selection()?;
    if let Some(key) = vivianne_providers::env::key_for(&selection.provider) {
        write_frame(
            &mut conn.writer,
            &ClientMessage::SessionCredential {
                provider: selection.provider.clone(),
                key,
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    let thread_id = uuid_like();
    write_frame(
        &mut conn.writer,
        &ClientMessage::RunPrompt {
            thread_id: thread_id.clone(),
            workspace,
            prompt,
            provider: selection.provider,
            model: selection.model,
            mode: mode.to_owned(),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    loop {
        let message: ServerMessage = read_frame(&mut conn.reader)
            .await
            .map_err(|e| e.to_string())?;
        match message {
            ServerMessage::RunEvent { event, .. } => match event {
                WireEvent::Text { text } => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                WireEvent::ToolStart { name, summary } => eprintln!("\n[{name}] {summary}"),
                WireEvent::ToolEnd {
                    name,
                    is_error: true,
                } => eprintln!("[{name} failed]"),
                WireEvent::Approval { id, title } => {
                    eprint!("\n[asks] {title}  allow? [y/N] ");
                    let mut answer = String::new();
                    std::io::stdin()
                        .lock()
                        .read_line(&mut answer)
                        .map_err(|e| e.to_string())?;
                    let allow = matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes");
                    write_frame(
                        &mut conn.writer,
                        &ClientMessage::ApprovalAnswer {
                            thread_id: thread_id.clone(),
                            id,
                            allow,
                        },
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                WireEvent::Done | WireEvent::Stopped => {
                    println!();
                    return Ok(());
                }
                WireEvent::Failed { message } => return Err(message),
                _ => {}
            },
            ServerMessage::Error { message, .. } => return Err(message),
            ServerMessage::Ping { id } => {
                write_frame(&mut conn.writer, &ClientMessage::Pong { id })
                    .await
                    .map_err(|e| e.to_string())?
            }
            ServerMessage::RunState {
                state: AgentState::Blocked,
                ..
            } => {}
            _ => {}
        }
    }
}

fn uuid_like() -> String {
    // 128 random-ish bits from the clock and the process; enough to name a thread, which is not a secret.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("{nanos:x}-{:x}", std::process::id())
}
