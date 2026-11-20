//! Headless chat through the runtime: `viv [prompt]`.
//! Proves the backend works with no UI. Configuration is by environment,
//! see `vivianne_providers::env`.

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

async fn run() -> Result<(), String> {
    let model = env::model()?;
    let workdir = std::env::current_dir().map_err(|e| e.to_string())?;
    let agent = Agent::new(
        env::provider()?,
        &model,
        Some(coding_system_prompt(&workdir)),
        Toolbox::coding(&workdir),
    );
    let mut history = Vec::new();

    let prompt = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
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
        AgentEvent::Retrying { attempt, max_attempts, delay, reason } => {
            eprintln!("\n[retry {attempt}/{max_attempts} in {}s] {reason}", delay.as_secs());
        }
        AgentEvent::Fallback { to, reason, .. } => eprintln!("\n[switched to {to}] {reason}"),
        AgentEvent::Compacted { before, after } => eprintln!("\n[condensed the conversation: {before} to {after} tokens]"),
    }
}
