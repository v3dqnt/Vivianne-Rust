//! The tools an agent uses to hand work to subagents and collect it: `task`, `task_output`, `wait_tasks`,
//! `kill_task` and `list_subagent_profiles`.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::Mode;
use vivianne_tools::{Tool, ToolResult};

use super::profiles::{self, Profile};
use super::subagents::{self, Registry, Start};
use super::{AgentSpec, caller, spawner};

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Longest a wait may be, and how long one is when the call does not say.
const MAX_WAIT_SECS: u64 = 900;
const DEFAULT_WAIT_SECS: u64 = 120;

fn wait_of(args: &Value, default: u64) -> Duration {
    Duration::from_secs(
        args.get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(default)
            .min(MAX_WAIT_SECS),
    )
}

/// Starts a subagent.
#[derive(Debug, Default)]
pub struct Task;

#[async_trait]
impl Tool for Task {
    fn name(&self) -> &str {
        "task"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "task",
            "description": "Hand a piece of work to a subagent: another agent with its own conversation, working in this project. Use it to search or review in parallel, or to keep a big job's detail out of this conversation. It starts in the background by default: carry on, then collect the answer with task_output or wait_tasks. A subagent changes files only when you are in Auto mode; otherwise it can only look. Subagents cannot start subagents.",
            "parameters": {
                "type": "object",
                "properties": {
                    "description": { "type": "string", "description": "A few words on what it is for." },
                    "prompt": { "type": "string", "description": "What to do, in full: it knows nothing of this conversation. Optional only for a subagent_type that has a preset task (list_subagent_profiles)." },
                    "subagent_type": { "type": "string", "description": "A kind from list_subagent_profiles. Default general-purpose." },
                    "name": { "type": "string", "description": "A name to call it by: lower-case letters, digits and dashes. Default agent-1, agent-2..." },
                    "context": { "type": "string", "description": "Extra facts added to the prompt." },
                    "run_in_background": { "type": "boolean", "description": "Default true. False waits here for the answer." }
                },
                "required": ["description"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let (Some(caller), Some(spawner)) = (caller(), spawner()) else {
            return ToolResult::failure("task works inside a conversation's own agent.");
        };
        if caller.depth >= 1 {
            return ToolResult::failure("you are a subagent already: do the work yourself.");
        }
        let Some(description) = text(args, "description") else {
            return ToolResult::failure("'description' is required.");
        };
        let kind = text(args, "subagent_type").unwrap_or("general-purpose");
        // A name that matches no profile is just a label: the work is general-purpose.
        let profile = profiles::find(kind).unwrap_or_else(|| Profile {
            name: kind.to_owned(),
            description: String::new(),
            read_only: false,
            system: String::new(),
            default_prompt: None,
            builtin: false,
        });
        let context = text(args, "context");
        let Some(base) = text(args, "prompt").or(profile.default_prompt.as_deref()) else {
            return ToolResult::failure(format!(
                "no 'prompt' was given and '{kind}' has no preset task. Say what to do, or pick a kind with one (list_subagent_profiles)."
            ));
        };
        let prompt = context.map_or_else(|| base.to_owned(), |c| format!("{base}\n\nContext: {c}"));
        let registry = Registry::global();
        let name = match text(args, "name") {
            Some(n) if !subagents::valid_name(n) => {
                return ToolResult::failure(format!(
                    "'{n}' is not a valid name: use lower-case letters, digits and dashes, starting with a letter, up to 32 characters, and not 'lead'."
                ));
            }
            Some(n) if registry.get(&caller.thread, n).is_some() => {
                return ToolResult::failure(format!(
                    "a subagent called '{n}' exists already in this conversation."
                ));
            }
            Some(n) => n.to_owned(),
            None => registry.next_name(&caller.thread),
        };
        // It may change files only when its parent is free to: anything less looks and reports.
        let mode = if profile.read_only || caller.mode != Mode::Auto {
            Mode::Plan
        } else {
            Mode::Auto
        };
        let spec = AgentSpec {
            folder: caller.folder.clone(),
            mode,
            system: profile.system.clone(),
            model: caller.model.clone(),
        };
        let request = Start {
            name,
            description: description.to_owned(),
            profile: profile.name,
            prompt,
            spec,
        };
        let handle = match subagents::start(spawner.as_ref(), &caller, request) {
            Ok(handle) => handle,
            Err(why) => return ToolResult::failure(why),
        };
        if args
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let reading = if mode == Mode::Plan {
                "read-only"
            } else {
                "able to change files"
            };
            return ToolResult::ok(format!(
                "Started {} ({}), {reading}: {description}. It works in the background: collect its answer with task_output or wait_tasks.",
                handle.name, handle.id
            ));
        }
        handle.wait(Duration::from_secs(MAX_WAIT_SECS)).await;
        ToolResult::ok(handle.report())
    }
}

/// Reads a subagent's answer, waiting for it if asked.
#[derive(Debug, Default)]
pub struct TaskOutput;

#[async_trait]
impl Tool for TaskOutput {
    fn name(&self) -> &str {
        "task_output"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "task_output",
            "description": "Get what a subagent you started has answered so far: its answer when it is done, or how it is going. With wait, holds until it finishes.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Its name or id." },
                    "wait": { "type": "boolean", "description": "Hold until it is done. Default false." },
                    "timeout_secs": { "type": "integer", "description": "Longest to wait. Default 120, at most 900." }
                },
                "required": ["task_id"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(caller) = caller() else {
            return ToolResult::failure("no conversation to look in.");
        };
        let id = text(args, "task_id").unwrap_or_default();
        let Some(handle) = Registry::global().get(&caller.thread, id) else {
            return ToolResult::failure(format!("no subagent '{id}' in this conversation."));
        };
        if args.get("wait").and_then(Value::as_bool).unwrap_or(false) {
            handle.wait(wait_of(args, DEFAULT_WAIT_SECS)).await;
        }
        ToolResult::ok(format!("{}\n\n{}", handle.summary(), handle.report()))
    }
}

/// Waits for subagents to finish.
#[derive(Debug, Default)]
pub struct WaitTasks;

#[async_trait]
impl Tool for WaitTasks {
    fn name(&self) -> &str {
        "wait_tasks"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "wait_tasks",
            "description": "Wait until the subagents you started are done (all of them, or the ones named), then give each one's answer. Use it when you have nothing left to do but wait.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task_ids": { "type": "array", "items": { "type": "string" }, "description": "Names or ids. Default: every one still working." },
                    "timeout_secs": { "type": "integer", "description": "Longest to wait. Default 120, at most 900." }
                }
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(caller) = caller() else {
            return ToolResult::failure("no conversation to look in.");
        };
        let registry = Registry::global();
        let named: Vec<String> = args
            .get("task_ids")
            .and_then(Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let handles = if named.is_empty() {
            registry
                .of(&caller.thread)
                .into_iter()
                .filter(|h| !h.status().is_done())
                .collect()
        } else {
            let mut found = Vec::new();
            for id in &named {
                match registry.get(&caller.thread, id) {
                    Some(h) => found.push(h),
                    None => {
                        return ToolResult::failure(format!(
                            "no subagent '{id}' in this conversation."
                        ));
                    }
                }
            }
            found
        };
        if handles.is_empty() {
            return ToolResult::ok("Nothing is working.");
        }
        let limit = wait_of(args, DEFAULT_WAIT_SECS);
        futures::future::join_all(handles.iter().map(|h| h.wait(limit))).await;
        ToolResult::ok(
            handles
                .iter()
                .map(|h| format!("## {}\n{}", h.summary(), h.report()))
                .collect::<Vec<_>>()
                .join("\n\n"),
        )
    }
}

/// Stops a subagent.
#[derive(Debug, Default)]
pub struct KillTask;

#[async_trait]
impl Tool for KillTask {
    fn name(&self) -> &str {
        "kill_task"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "kill_task",
            "description": "Stop a subagent you started that is still working.",
            "parameters": {
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Its name or id." },
                    "reason": { "type": "string" }
                },
                "required": ["task_id"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(caller) = caller() else {
            return ToolResult::failure("no conversation to look in.");
        };
        let id = text(args, "task_id").unwrap_or_default();
        let Some(handle) = Registry::global().get(&caller.thread, id) else {
            return ToolResult::failure(format!("no subagent '{id}' in this conversation."));
        };
        if handle.kill(text(args, "reason").map(str::to_owned)) {
            ToolResult::ok(format!("Stopped {}.", handle.name))
        } else {
            ToolResult::ok(format!("{} had finished already.", handle.name))
        }
    }
}

/// Says what kinds of subagent there are.
#[derive(Debug, Default)]
pub struct ListSubagentProfiles;

#[async_trait]
impl Tool for ListSubagentProfiles {
    fn name(&self) -> &str {
        "list_subagent_profiles"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "list_subagent_profiles",
            "description": "List the kinds of subagent task can start (its subagent_type), what each is for, and which ones only look.",
            "parameters": { "type": "object", "properties": {} }
        }))
    }

    async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
        ToolResult::ok(
            profiles::profiles()
                .iter()
                .map(|p| {
                    format!(
                        "- {}{}{}: {}",
                        p.name,
                        if p.read_only { " (read-only)" } else { "" },
                        if p.default_prompt.is_some() {
                            " (has a preset task)"
                        } else {
                            ""
                        },
                        p.description
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}
