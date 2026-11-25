//! Threads that talk to each other: message one, ask one and wait for its answer, or start a new one. A message to
//! a thread that is idle wakes it; to one that is busy, it waits as the next thing that thread does. Chains are
//! capped (a message passed on and on stops after `MAX_HOPS`, a thread started by a thread goes `MAX_DEPTH`
//! levels down). The threads are reached through the [`ThreadHub`](super::ThreadHub) the app provides.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_tools::{Tool, ToolResult};

use super::{Delivered, MAX_HOPS, caller, child_depth, hub, next_hop, record_depth};
use crate::store;

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn title_of(id: &str) -> String {
    store::load(id).map_or_else(|| "another conversation".to_owned(), |t| t.title)
}

/// What the thread that receives a message is told, so it knows who is speaking and can answer.
fn frame(from: &str, hop: u32, body: &str, expects_answer: bool) -> String {
    let how = if expects_answer {
        "Your next reply goes back to it as the answer."
    } else {
        "To answer, use message_thread with its id. Do that only when it moves the work forward."
    };
    format!(
        "Message from another conversation, \"{}\" (id {from}, hop {hop} of {MAX_HOPS}):\n\n{body}\n\n({how})",
        title_of(from)
    )
}

/// How long `ask_thread` waits for an answer when the call does not say.
const DEFAULT_ASK_SECS: u64 = 300;
const MAX_ASK_SECS: u64 = 900;

/// Sends a message to another thread.
#[derive(Debug, Default)]
pub struct MessageThread;

#[async_trait]
impl Tool for MessageThread {
    fn name(&self) -> &str {
        "message_thread"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "message_thread",
            "description": "Send a message to another of the user's conversations in this project (ids from list_threads). If it is idle it starts working on the message; if it is busy, the message is the next thing it does. Returns at once. To wait for its answer use ask_thread.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The conversation's id." },
                    "text": { "type": "string", "description": "What to tell it, in full: it has not seen this conversation." }
                },
                "required": ["id", "text"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let (Some(caller), Some(hub)) = (caller(), hub()) else {
            return ToolResult::failure("no other conversations can be reached from here.");
        };
        let (Some(to), Some(body)) = (text(args, "id"), text(args, "text")) else {
            return ToolResult::failure("'id' and 'text' are required.");
        };
        if to == caller.thread {
            return ToolResult::failure("that is this conversation.");
        }
        let hop = match next_hop(&caller.thread, to) {
            Ok(hop) => hop,
            Err(why) => return ToolResult::failure(why),
        };
        match hub
            .deliver(to, frame(&caller.thread, hop, body, false))
            .await
        {
            Ok(Delivered::Woke) => ToolResult::ok(format!(
                "Sent. \"{}\" was idle and has started on it.",
                title_of(to)
            )),
            Ok(Delivered::Queued) => ToolResult::ok(format!(
                "Sent. \"{}\" is busy: it will do this next.",
                title_of(to)
            )),
            Err(why) => ToolResult::failure(why),
        }
    }
}

/// Asks another thread and waits for its answer.
#[derive(Debug, Default)]
pub struct AskThread;

#[async_trait]
impl Tool for AskThread {
    fn name(&self) -> &str {
        "ask_thread"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "ask_thread",
            "description": "Ask another of the user's conversations in this project something and wait for what it replies. It works on the question like any message; its next reply comes back here.",
            "parameters": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The conversation's id." },
                    "text": { "type": "string", "description": "The question, in full: it has not seen this conversation." },
                    "timeout_secs": { "type": "integer", "description": "Longest to wait. Default 300, at most 900." }
                },
                "required": ["id", "text"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let (Some(caller), Some(hub)) = (caller(), hub()) else {
            return ToolResult::failure("no other conversations can be reached from here.");
        };
        let (Some(to), Some(body)) = (text(args, "id"), text(args, "text")) else {
            return ToolResult::failure("'id' and 'text' are required.");
        };
        if to == caller.thread {
            return ToolResult::failure("that is this conversation.");
        }
        let hop = match next_hop(&caller.thread, to) {
            Ok(hop) => hop,
            Err(why) => return ToolResult::failure(why),
        };
        let wait = Duration::from_secs(
            args.get("timeout_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_ASK_SECS)
                .min(MAX_ASK_SECS),
        );
        match hub
            .ask(to, frame(&caller.thread, hop, body, true), wait)
            .await
        {
            Ok(reply) => ToolResult::ok(format!("\"{}\" replied:\n\n{reply}", title_of(to))),
            Err(why) => ToolResult::failure(why),
        }
    }
}

/// Starts another thread.
#[derive(Debug, Default)]
pub struct SpawnThread;

#[async_trait]
impl Tool for SpawnThread {
    fn name(&self) -> &str {
        "spawn_thread"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "spawn_thread",
            "description": "Start a new conversation in this project, working on a prompt. It appears for the user as a thread of its own, and runs in parallel with this one. Name a coding agent in `agent` to have that agent do the work (see the agents the user has set up); otherwise it runs on Vivianne's own agent. Then use ask_thread to wait for it, or read_thread to see how it is going.",
            "parameters": {
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "What it should do, in full: it has not seen this conversation." },
                    "title": { "type": "string", "description": "A short name for the thread." },
                    "agent": { "type": "string", "description": "A coding agent's id, to run the thread on it. Optional." }
                },
                "required": ["prompt"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let (Some(caller), Some(hub)) = (caller(), hub()) else {
            return ToolResult::failure("no other conversations can be started from here.");
        };
        let Some(prompt) = text(args, "prompt") else {
            return ToolResult::failure("'prompt' is required.");
        };
        let depth = match child_depth(&caller.thread) {
            Ok(depth) => depth,
            Err(why) => return ToolResult::failure(why),
        };
        let title = text(args, "title").map(str::to_owned);
        let agent = text(args, "agent").map(str::to_owned);
        match hub.spawn(title, prompt.to_owned(), agent).await {
            Ok(thread) => {
                record_depth(&thread.id, depth);
                ToolResult::ok(format!(
                    "Started \"{}\" (id {}). It is working. Use ask_thread to wait for its answer, or read_thread to see how it is going.",
                    thread.title, thread.id
                ))
            }
            Err(why) => ToolResult::failure(why),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_says_who_it_is_from_and_how_it_may_be_answered() {
        let asked = frame("abc", 2, "what broke?", true);
        assert!(
            asked.contains("id abc")
                && asked.contains("hop 2 of")
                && asked.contains("goes back to it"),
            "{asked}"
        );
        let told = frame("abc", 1, "fyi", false);
        assert!(told.contains("message_thread"), "{told}");
    }
}
