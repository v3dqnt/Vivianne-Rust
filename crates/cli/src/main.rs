//! Headless chat through the runtime: `viv [prompt]`.
//! Proves the backend works with no UI. Configuration is by environment:
//!
//! - `VIV_MODEL` (required)
//! - `VIV_PROVIDER`: `anthropic` (default when `ANTHROPIC_API_KEY` is set) or `openai`
//! - `ANTHROPIC_API_KEY`; `OPENAI_API_KEY` (optional for local servers), `OPENAI_BASE_URL`

use std::io::{BufRead, Write};
use std::sync::Arc;

use tokio::sync::mpsc::unbounded_channel;
use vivianne_core::{Message, Role};
use vivianne_providers::Provider;
use vivianne_providers::anthropic::Anthropic;
use vivianne_providers::openai::OpenAiCompatible;
use vivianne_runtime::{Agent, AgentEvent};
use vivianne_tools::Toolbox;

const SYSTEM: &str =
    "You are Vivianne, a coding agent. Use the tools to inspect and change the project. Be brief.";

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("viv: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let model = std::env::var("VIV_MODEL").map_err(|_| "set VIV_MODEL")?;
    let workdir = std::env::current_dir().map_err(|e| e.to_string())?;
    let agent = Agent::new(
        provider()?,
        &model,
        Some(SYSTEM.to_owned()),
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

fn provider() -> Result<Arc<dyn Provider>, String> {
    let anthropic_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let wanted = std::env::var("VIV_PROVIDER").unwrap_or_else(|_| {
        if anthropic_key.is_some() {
            "anthropic"
        } else {
            "openai"
        }
        .to_owned()
    });
    match wanted.as_str() {
        "anthropic" => Ok(Arc::new(Anthropic::new(
            anthropic_key.ok_or("set ANTHROPIC_API_KEY")?,
        ))),
        "openai" => {
            let base = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_owned());
            Ok(Arc::new(OpenAiCompatible::new(
                "openai",
                &base,
                std::env::var("OPENAI_API_KEY").ok(),
            )))
        }
        other => Err(format!("unknown VIV_PROVIDER `{other}`")),
    }
}

async fn ask(agent: &Agent, history: &mut Vec<Message>, prompt: String) -> Result<(), String> {
    history.push(Message::new(Role::User, prompt));
    let (tx, mut rx) = unbounded_channel();
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            print_event(&event);
        }
    });
    let result = agent.run(history, &tx).await;
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
        AgentEvent::ToolEnd { .. } | AgentEvent::Done => {}
        AgentEvent::StepLimit => eprintln!("\n[stopped: step limit]"),
    }
}
