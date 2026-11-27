//! The tools an agent uses to hand work to subagents and collect it: `task`, `task_output`, `wait_tasks`,
//! `kill_task` and `list_subagent_profiles`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::Mode;
use vivianne_tools::{Tool, ToolResult};

use super::coding::{CodingJob, Installed};
use super::profiles::{self, Profile};
use super::subagents::{self, Registry, Start};
use super::{AgentSpec, Caller, caller, spawner};

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
                    "agent": { "type": "string", "description": "Have a coding agent installed on this machine do the work instead: its id in the Agent Client Protocol registry, such as claude-acp or codex-acp. Only in Auto mode, and not while a sandbox mode is set." },
                    "run_in_background": { "type": "boolean", "description": "Default true. False waits here for the answer." }
                },
                "required": ["description"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(caller) = caller() else {
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
        let background = args
            .get("run_in_background")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if let Some(agent) = text(args, "agent") {
            if let Err(why) = may_delegate(&caller) {
                return ToolResult::failure(why);
            }
            let job = CodingJob {
                agent: agent.to_owned(),
                folder: caller.folder.clone(),
                prompt: if profile.system.is_empty() {
                    prompt
                } else {
                    format!("{}\n\n{prompt}", profile.system)
                },
                read_only: profile.read_only,
            };
            let reading = if job.read_only {
                "asked only to look"
            } else {
                "able to change files"
            };
            let started = subagents::start_coding(
                Arc::new(Installed),
                &caller,
                name,
                description.to_owned(),
                job,
            );
            return match started {
                Ok(handle) => answer(&handle, background, reading, description).await,
                Err(why) => ToolResult::failure(why),
            };
        }
        let Some(spawner) = spawner() else {
            return ToolResult::failure("subagents are not set up here.");
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
            provider: caller.provider.clone(),
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
        let reading = if mode == Mode::Plan {
            "read-only"
        } else {
            "able to change files"
        };
        answer(&handle, background, reading, description).await
    }
}

/// What `task` says once its subagent is going: where to collect it, or, when it was to wait, its answer.
async fn answer(
    handle: &subagents::Handle,
    background: bool,
    reading: &str,
    description: &str,
) -> ToolResult {
    if background {
        return ToolResult::ok(format!(
            "Started {} ({}), {reading}: {description}. It works in the background: collect its answer with task_output or wait_tasks.",
            handle.name, handle.id
        ));
    }
    handle.wait(Duration::from_secs(MAX_WAIT_SECS)).await;
    ToolResult::ok(handle.report())
}

/// A coding agent runs its own tools in its own process: nothing here holds it to a mode or asks for it. So it is
/// started only when the person has said to act without asking, and never while a sandbox mode is set.
fn may_delegate(caller: &Caller) -> Result<(), String> {
    if caller.mode != Mode::Auto {
        return Err("a coding agent can only be started in Auto mode; ask the user to switch, or use a subagent without 'agent'.".to_owned());
    }
    if caller.confined {
        return Err("a coding agent cannot be held to a sandbox mode, so it is not started while one is set; use a subagent without 'agent'.".to_owned());
    }
    Ok(())
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
        // What `task`'s `agent` may name: only what is set up here.
        let ready: Vec<String> = vivianne_providers::acp::harnesses()
            .await
            .into_iter()
            .filter(|h| h.available && !h.needs_setup())
            .map(|h| h.id)
            .collect();
        let agents = if ready.is_empty() {
            "none".to_owned()
        } else {
            ready.join(", ")
        };
        let kinds = profiles::profiles()
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
            .join("\n");
        ToolResult::ok(format!(
            "{kinds}\n\nCoding agents set up here (task's agent): {agents}"
        ))
    }
}

#[cfg(test)]
mod delegate_tests {
    use serde_json::json;

    use super::*;
    use crate::orchestration::scope;

    async fn refused(mode: Mode, confined: bool, thread: &str) -> String {
        let caller = Caller {
            thread: thread.to_owned(),
            mode,
            folder: std::env::temp_dir(),
            model: None,
            provider: None,
            confined,
            depth: 0,
        };
        let args = json!({ "description": "d", "prompt": "p", "agent": "claude-acp" });
        let out = scope(caller, Task.execute(&args, "")).await;
        assert!(out.is_error, "{}", out.content);
        assert!(Registry::global().of(thread).is_empty());
        out.content
    }

    #[tokio::test]
    async fn a_coding_agent_is_not_started_unless_the_person_acts_without_asking() {
        assert!(
            refused(Mode::Ask, false, "d-ask")
                .await
                .contains("Auto mode")
        );
        assert!(
            refused(Mode::Plan, false, "d-plan")
                .await
                .contains("Auto mode")
        );
        assert!(
            refused(Mode::Auto, true, "d-confined")
                .await
                .contains("sandbox mode")
        );
    }
}

#[cfg(test)]
mod real_agent_tests {
    use serde_json::json;

    use super::*;
    use crate::orchestration::scope;

    /// Spends the person's quota with that agent, so it runs only when asked for by name:
    /// `VIV_TEST_CODING_AGENT=claude-acp cargo test -p vivianne-runtime a_real_coding_agent -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs a signed-in coding agent and spends its quota"]
    async fn a_real_coding_agent_does_a_task() {
        let Ok(agent) = std::env::var("VIV_TEST_CODING_AGENT") else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("viv-real-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "The code word is TANGERINE.\n").unwrap();
        let caller = Caller {
            thread: format!("real-agent-{}", std::process::id()),
            mode: Mode::Auto,
            folder: dir.clone(),
            model: None,
            provider: None,
            confined: false,
            depth: 0,
        };
        let args = json!({
            "description": "copy the code word",
            "prompt": "Read note.txt in the current folder. Write only the code word from it into a new file named answer.txt in the same folder. Do nothing else. Then reply with the word.",
            "agent": agent,
            "run_in_background": false
        });
        let started = std::time::Instant::now();
        let out = scope(caller.clone(), Task.execute(&args, "")).await;
        println!("took {}s", started.elapsed().as_secs());
        println!("result: {}", out.content);
        for handle in Registry::global().of(&caller.thread) {
            println!("{}", handle.summary());
        }
        let wrote = std::fs::read_to_string(dir.join("answer.txt")).ok();
        println!("answer.txt: {wrote:?}");
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| Some(e.ok()?.file_name()))
            .collect();
        println!("files: {files:?}");
        if wrote.is_none() {
            for line in vivianne_providers::acp::trace_lines()
                .iter()
                .rev()
                .take(25)
                .rev()
            {
                println!("trace: {line}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(wrote.as_deref().map(str::trim), Some("TANGERINE"));
    }
}
