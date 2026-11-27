//! `viv`: headless chat through the runtime, and the machine commands.
//!
//! - `viv [prompt]` chats here. Configuration is by environment, see `vivianne_providers::env`.
//! - `viv machine ...` saves and checks machines; `viv --machine <label> --workspace <dir> <prompt>` runs a prompt on
//!   one; `viv [--session S] server ...` and `viv [--session S] remote-client-bridge` are the machine's side.

mod inspect;
mod remote_cli;
mod thread_query;

use std::io::{BufRead, Write};

use tokio::sync::mpsc::unbounded_channel;
use vivianne_core::{Message, Role, Thinking};
use vivianne_providers::env;
use vivianne_runtime::{Agent, AgentEvent, coding_system_prompt};
use vivianne_tools::Toolbox;

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("viv: {message}");
        std::process::exit(1);
    }
}

/// Removes `--flag value` from the arguments and returns the value.
fn take_flag(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    let value = args.get(at + 1).cloned()?;
    args.drain(at..=at + 1);
    Some(value)
}

async fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let session =
        vivianne_remote::socket::resolve_session(take_flag(&mut args, "--session").as_deref());
    if let Some(label) = take_flag(&mut args, "--machine") {
        return remote_cli::run_on_machine(&label, args).await;
    }
    // The read-only commands: a usage error exits 2, a failed check exits 1 (R5.2).
    let rest = || args[1..].to_vec();
    let inspected = match args.first().map(String::as_str) {
        Some("status") => Some(inspect::status(rest()).map(|()| true)),
        Some("doctor") => Some(inspect::doctor(rest())),
        Some("config") => Some(inspect::config_cmd(rest()).map(|()| true)),
        Some("sessions") => Some(inspect::sessions(rest()).map(|()| true)),
        Some("session") => Some(inspect::session(rest()).map(|()| true)),
        Some("profiles") => Some(inspect::profiles_cmd(rest()).map(|()| true)),
        Some("thread") => Some(inspect::thread_cmd(rest()).map(|()| true)),
        _ => None,
    };
    match inspected {
        Some(Ok(true)) => return Ok(()),
        Some(Ok(false)) => std::process::exit(1),
        Some(Err(inspect::Usage(why))) => {
            eprintln!("viv: {why}");
            std::process::exit(2);
        }
        None => {}
    }
    match args.first().map(String::as_str) {
        Some("machine") => return remote_cli::machine(args[1..].to_vec()).await,
        Some("server") => return remote_cli::server(&session, &args[1..]).await,
        Some("remote-client-bridge") => return remote_cli::bridge_host(&session, &args[1..]).await,
        _ => {}
    }
    chat_here(args).await
}

async fn chat_here(args: Vec<String>) -> Result<(), String> {
    let model = env::model()?;
    let workdir = std::env::current_dir().map_err(|e| e.to_string())?;
    let settings = vivianne_runtime::config::resolve(&workdir, None);
    vivianne_tools::results::set_limit(settings.max_tool_result_bytes.value);
    let mut agent = Agent::new(
        env::provider()?,
        &model,
        Some(coding_system_prompt(&workdir)),
        Toolbox::coding(&workdir),
    )
    .with_max_steps(settings.max_agent_steps.value)
    // The project's instruction files, the clock and the terminal's place, as the app adds them.
    .with_instructions(vec![workdir.display().to_string()]);
    // What each run changed, per file with line counts.
    if let Ok(recorder) =
        vivianne_runtime::changes::Recorder::new(&workdir, settings.change_limits())
    {
        agent = agent.with_changes(std::sync::Arc::new(recorder));
    }
    let mut history = Vec::new();

    let prompt = args.join(" ");
    if !prompt.is_empty() {
        return ask(&agent, &mut history, prompt).await;
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if !line.trim().is_empty() {
            ask(&agent, &mut history, line).await?;
        }
    }
    Ok(())
}

async fn ask(agent: &Agent, history: &mut Vec<Message>, prompt: String) -> Result<(), String> {
    history.push(Message::new(Role::User, prompt));
    let (tx, mut rx) = unbounded_channel();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            print_event(&event);
        }
    });
    let result = agent.run(history, &tx, Thinking::default()).await;
    drop(tx);
    let _ = printer.await;
    println!();
    result.map_err(|e| e.to_string())
}

fn print_event(event: &AgentEvent) {
    match event {
        AgentEvent::Text(piece) => {
            print!("{piece}");
            let _ = std::io::stdout().flush();
        }
        AgentEvent::ToolStart { name, args, .. } => eprintln!("\n[{name}] {args}"),
        AgentEvent::ToolEnd {
            is_error: true,
            output,
            ..
        } => eprintln!("[failed] {output}"),
        AgentEvent::ToolEnd { .. } | AgentEvent::Usage { .. } | AgentEvent::Done => {}
        AgentEvent::Approval { title, .. } => eprintln!("[asks] {title}"),
        AgentEvent::StepLimit => eprintln!("\n[stopped: step limit]"),
        AgentEvent::Stopped => eprintln!("\n[stopped]"),
        AgentEvent::Retrying {
            attempt,
            max_attempts,
            delay,
            reason,
        } => {
            eprintln!(
                "\n[retry {attempt}/{max_attempts} in {}s] {reason}",
                delay.as_secs()
            );
        }
        AgentEvent::Fallback { to, reason, .. } => eprintln!("\n[switched to {to}] {reason}"),
        AgentEvent::Compacted { before, after } => {
            eprintln!("\n[condensed the conversation: {before} to {after} tokens]");
        }
    }
}
