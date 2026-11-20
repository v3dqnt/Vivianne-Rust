//! What a coding agent gets from Vivianne over MCP: a short, fixed list of Vivianne's own tools (the project's
//! memory, the user's other threads in it, a language server), and the user's own MCP servers, so the agent has the
//! same connected tools Vivianne does.
//!
//! Not on the list, on purpose: anything that writes files or runs commands (the agent has its own, under its own
//! permission requests that Vivianne already answers), and secrets of any kind. Plan mode refuses a tool that is
//! not known to only look, as it does for Vivianne's own agent. A call outside a running turn is refused by the
//! protocol layer.

use std::path::Path;
use std::sync::Arc;

use vivianne_core::Mode;
use vivianne_providers::acp::{McpServerSpec, OfferedTool, OwnTools, ToolOutput};
use vivianne_tools::Toolbox;
use vivianne_tools::mcp::{McpServerConfig, McpTransportConfig};

/// The tools offered, with what each is for (the agent's instructions are written from this).
const OFFERED: [(&str, &str); 5] = [
    ("recall", "search the user's memory from earlier sessions"),
    ("remember", "save a lasting fact or correction to the user's memory"),
    ("list_threads", "list the user's other conversations in this project"),
    ("read_thread", "read another of the user's conversations in this project"),
    ("lsp", "ask a language server: definitions, references, types, symbols, and the errors in a file (read-only)"),
];

/// Longest answer a call may put into the agent's context.
const MAX_ANSWER_CHARS: usize = 100_000;

/// Vivianne's tools for one turn of an agent working in `folder`.
pub fn for_turn(folder: &Path, plan_mode: bool) -> OwnTools {
    let root = folder.display().to_string();
    let mut toolbox = Toolbox::coding(folder);
    if let Some(backend) = crate::backend::get() {
        vivianne_memory::agent::add_tools(&mut toolbox, &root, &backend.events, vivianne_ai::host::ModelRef::default());
    }
    toolbox.add(vivianne_runtime::thread_tools::ListThreads::new(&root));
    toolbox.add(vivianne_runtime::thread_tools::ReadThread::new(&root));
    let tools: Vec<OfferedTool> = toolbox
        .definitions()
        .into_iter()
        .filter_map(|d| {
            let (_, summary) = OFFERED.iter().find(|(name, _)| *name == d.name)?;
            Some(OfferedTool { name: d.name, summary: (*summary).to_owned(), description: d.description, schema: d.parameters })
        })
        .collect();
    let toolbox = Arc::new(toolbox);
    let offered: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
    let call = Arc::new(move |name: String, args: serde_json::Value| {
        let (toolbox, offered) = (toolbox.clone(), offered.clone());
        Box::pin(async move {
            // The protocol layer already refuses a name that is not on the list; this keeps the toolbox's other
            // tools (shell, write...) out of reach even if that ever changed.
            if !offered.contains(&name) {
                return ToolOutput::text(format!("`{name}` is not offered."), true);
            }
            if plan_mode && !Mode::Plan.allows(&name) {
                return ToolOutput::text(format!("`{name}` is not available in plan mode: it would change something."), true);
            }
            match toolbox.execute(&name, &args).await {
                Some(result) => ToolOutput::text(result.content.chars().take(MAX_ANSWER_CHARS).collect::<String>(), result.is_error),
                None => ToolOutput::text(format!("`{name}` is not available."), true),
            }
        }) as futures::future::BoxFuture<'static, ToolOutput>
    });
    OwnTools { tools, call }
}

/// The user's MCP servers as an agent takes them. Left out: a server signed in through Vivianne (its token is not
/// Vivianne's to pass on).
pub fn specs_of(configs: &[McpServerConfig]) -> Vec<McpServerSpec> {
    configs
        .iter()
        .filter_map(|c| match &c.transport {
            McpTransportConfig::Stdio { command, args, env } => Some(McpServerSpec::Stdio {
                name: c.name.clone(),
                command: command.clone(),
                args: args.clone(),
                env: env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            }),
            McpTransportConfig::Http { url, headers } if c.auth.is_none() => {
                Some(McpServerSpec::Http { name: c.name.clone(), url: url.clone(), headers: headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect() })
            }
            McpTransportConfig::Http { .. } => None,
        })
        .collect()
}

/// The servers connected right now, as specs.
pub async fn specs() -> Vec<McpServerSpec> {
    specs_of(&vivianne_tools::mcp::list_servers().await)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn only_the_fixed_list_is_offered_and_plan_mode_refuses_what_would_change_something() {
        let own = for_turn(Path::new("."), true);
        let names: Vec<&str> = own.tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"list_threads") && names.contains(&"read_thread") && names.contains(&"lsp"), "{names:?}");
        for never in ["shell", "write", "edit", "read", "grep", "repo_clone"] {
            assert!(!names.contains(&never), "{never} is not offered to another program");
        }
        assert!(own.tools.iter().all(|t| t.schema["type"] == "object" && !t.summary.is_empty()));
        let refused = (own.call)("shell".into(), json!({ "command": "echo hi" })).await;
        assert!(refused.is_error, "{refused:?}");
        let allowed = (own.call)("list_threads".into(), json!({})).await;
        assert!(!allowed.is_error, "{allowed:?}");
        // Another turn that is not plan mode may use what changes things; here, the same refusal for a name that is
        // not on the list at all.
        let free = for_turn(Path::new("."), false);
        assert!((free.call)("write".into(), json!({})).await.is_error);
    }

    #[test]
    fn the_users_servers_go_along_except_one_signed_in_through_vivianne() {
        let configs = [
            McpServerConfig { name: "mem".into(), transport: McpTransportConfig::Stdio { command: "npx".into(), args: vec!["-y".into(), "x".into()], env: HashMap::new() }, timeout_ms: None, auth: None },
            McpServerConfig { name: "open".into(), transport: McpTransportConfig::Http { url: "https://a.test/mcp".into(), headers: HashMap::new() }, timeout_ms: None, auth: None },
            McpServerConfig { name: "signed".into(), transport: McpTransportConfig::Http { url: "https://b.test/mcp".into(), headers: HashMap::new() }, timeout_ms: None, auth: Some("signed".into()) },
        ];
        let specs = specs_of(&configs);
        let names: Vec<&str> = specs.iter().map(|s| match s { McpServerSpec::Stdio { name, .. } | McpServerSpec::Http { name, .. } => name.as_str() }).collect();
        assert_eq!(names, ["mem", "open"]);
    }
}
