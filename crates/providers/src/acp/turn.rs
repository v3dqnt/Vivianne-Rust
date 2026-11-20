//! One turn with an agent: send the user's message, turn what the agent streams back into [`AcpEvent`]s.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::conn::{Conn, Incoming, trace};
use super::registry::Harness;

/// Opening a session can wait on a first download.
const SESSION_TIMEOUT: Duration = Duration::from_mins(2);

/// What the agent is doing, as the chat can draw it. The agent runs its own tools, so tool calls here are things
/// that happen, not things to run.
#[derive(Debug)]
pub enum AcpEvent {
    Text(String),
    Thought(String),
    ToolStart { id: String, name: String, args: Value },
    ToolEnd { id: String, name: String, args: Value, output: String, is_error: bool },
    /// The agent's plan, as a checklist (`[x]` done, `[~]` doing, `[ ]` to do).
    Plan(String),
    /// The agent wants to do something and asks first. It waits until this is answered.
    Permission(PermissionRequest),
}

/// A tool call as it is shown when asking.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug)]
pub struct PermissionRequest {
    pub tool: ToolInfo,
    /// What to tell the user, for example "Claude Code wants to: run the tests".
    pub why: Option<String>,
    reply: oneshot::Sender<bool>,
}

impl PermissionRequest {
    /// Answers the agent. Dropping the request without answering refuses.
    pub fn answer(self, allow: bool) {
        let _ = self.reply.send(allow);
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Finished,
    /// The user stopped it.
    Stopped,
    /// The agent declined to continue.
    Refused,
    /// The agent ran into its own limit; the text says which.
    Limit(String),
}

/// A piece of what the user sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptPart {
    Text(String),
    Image { base64: String, mime: String },
}

fn prompt_json(parts: &[PromptPart]) -> Vec<Value> {
    parts
        .iter()
        .map(|part| match part {
            PromptPart::Text(text) => json!({ "type": "text", "text": text }),
            PromptPart::Image { base64, mime } => json!({ "type": "image", "data": base64, "mimeType": mime }),
        })
        .collect()
}

/// The tool name the chat draws for an agent's tool call kind.
fn tool_name(kind: &str) -> &'static str {
    match kind {
        "read" => "read",
        "edit" => "edit",
        "delete" | "move" => "write",
        "search" => "grep",
        "execute" => "bash",
        "fetch" => "web_fetch",
        "think" => "think",
        _ => "tool",
    }
}

type Diff = (String, Option<String>, String);

/// A tool call's content as text, and the first diff in it.
fn tool_content(content: Option<&Value>) -> (String, Option<Diff>) {
    let mut text = Vec::new();
    let mut diff = None;
    for block in content.and_then(Value::as_array).into_iter().flatten() {
        match block.get("type").and_then(Value::as_str) {
            Some("content") => {
                if let Some(t) = block.pointer("/content/text").and_then(Value::as_str) {
                    text.push(t.to_owned());
                }
            }
            Some("diff") => {
                let path = block.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
                let old = block.get("oldText").and_then(Value::as_str).map(str::to_owned);
                let new = block.get("newText").and_then(Value::as_str).unwrap_or_default().to_owned();
                text.push(format!("Edited {path}"));
                diff.get_or_insert((path, old, new));
            }
            Some("terminal") => text.push("(terminal output)".to_owned()),
            _ => {}
        }
    }
    (text.join("\n"), diff)
}

/// The agent's plan as a checklist.
fn plan_text(entries: &[Value]) -> String {
    entries
        .iter()
        .map(|e| {
            let mark = match e.get("status").and_then(Value::as_str) {
                Some("completed") => "x",
                Some("in_progress") => "~",
                _ => " ",
            };
            format!("[{mark}] {}", e.get("content").and_then(Value::as_str).unwrap_or_default())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call being built up from `tool_call` and `tool_call_update` messages.
struct Call {
    name: &'static str,
    args: Value,
    started: bool,
}

/// Turns the agent's updates into events, remembering the tool calls in flight.
#[derive(Default)]
struct Updates {
    calls: HashMap<String, Call>,
}

impl Updates {
    fn apply(&mut self, update: &Value) -> Vec<AcpEvent> {
        match update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default() {
            "agent_message_chunk" => update.pointer("/content/text").and_then(Value::as_str).map(|t| AcpEvent::Text(t.to_owned())).into_iter().collect(),
            "agent_thought_chunk" => update.pointer("/content/text").and_then(Value::as_str).map(|t| AcpEvent::Thought(t.to_owned())).into_iter().collect(),
            "tool_call" | "tool_call_update" => self.tool(update),
            "plan" => {
                let entries = update.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
                vec![AcpEvent::Plan(plan_text(&entries))]
            }
            _ => Vec::new(),
        }
    }

    fn tool(&mut self, u: &Value) -> Vec<AcpEvent> {
        let Some(id) = u.get("toolCallId").and_then(Value::as_str).map(str::to_owned) else { return Vec::new() };
        let call = self.calls.entry(id.clone()).or_insert_with(|| Call { name: "tool", args: json!({}), started: false });
        if let Some(kind) = u.get("kind").and_then(Value::as_str) {
            call.name = tool_name(kind);
        }
        if let Some(raw) = u.get("rawInput").filter(|v| v.is_object()) {
            call.args = raw.clone();
        }
        let name = call.name;
        if let (Some(title), Some(args)) = (u.get("title").and_then(Value::as_str), call.args.as_object_mut()) {
            args.entry("title").or_insert_with(|| json!(title));
            // What the chat's tool rows read: a command, a path.
            if name == "bash" {
                args.entry("command").or_insert_with(|| json!(title));
            }
        }
        if let (Some(path), Some(args)) = (u.pointer("/locations/0/path").and_then(Value::as_str), call.args.as_object_mut()) {
            args.entry("path").or_insert_with(|| json!(path));
        }
        let mut events = Vec::new();
        if !call.started {
            call.started = true;
            events.push(AcpEvent::ToolStart { id: id.clone(), name: name.to_owned(), args: call.args.clone() });
        }
        let status = u.get("status").and_then(Value::as_str).unwrap_or_default();
        if status == "completed" || status == "failed" {
            events.push(self.finish(&id, u, status == "failed"));
        }
        events
    }

    /// The end of a tool call, with a diff drawn as an edit (before and after).
    fn finish(&mut self, id: &str, u: &Value, is_error: bool) -> AcpEvent {
        let call = self.calls.remove(id).unwrap_or(Call { name: "tool", args: json!({}), started: true });
        let (body, diff) = tool_content(u.get("content"));
        let mut args = call.args;
        let name = if let Some((path, old, new)) = diff {
            if let Some(map) = args.as_object_mut() {
                map.insert("path".to_owned(), json!(path));
                map.insert("old_string".to_owned(), json!(old.unwrap_or_default()));
                map.insert("new_string".to_owned(), json!(new));
            }
            "edit"
        } else {
            call.name
        };
        let output = if body.is_empty() {
            u.get("rawOutput").map_or_else(|| if is_error { "Failed.".to_owned() } else { "Done.".to_owned() }, ToString::to_string)
        } else {
            body
        };
        AcpEvent::ToolEnd { id: id.to_owned(), name: name.to_owned(), args, output, is_error }
    }

    /// Ends the calls a stop or a failure left open.
    fn stop_open(&mut self) -> Vec<AcpEvent> {
        self.calls
            .drain()
            .filter(|(_, call)| call.started)
            .map(|(id, call)| AcpEvent::ToolEnd { id, name: call.name.to_owned(), args: call.args, output: "Stopped.".to_owned(), is_error: true })
            .collect()
    }
}

/// What the user can do when a session won't open: usually sign in.
fn explain_start_failure(harness: &Harness, error: &str) -> String {
    let low = error.to_lowercase();
    if ["auth", "login", "log in", "api key"].iter().any(|word| low.contains(word)) {
        let how = match harness.id.as_str() {
            "claude-acp" => " Run `claude` once in a terminal and sign in (or set ANTHROPIC_API_KEY).",
            "codex-acp" => " Run `codex login` in a terminal (or set OPENAI_API_KEY).",
            "gemini" => " Run `gemini` once in a terminal and sign in (or set GEMINI_API_KEY).",
            _ => " Sign in to it once in a terminal.",
        };
        format!("{} needs you to sign in.{how}\n\n({error})", harness.name)
    } else {
        format!("{} couldn't start a session: {error}", harness.name)
    }
}

/// Opens the thread's session on its first turn and keeps it for the rest.
pub(super) async fn session_of(conn: &Conn, harness: &Harness, cwd: &str) -> Result<String, String> {
    let mut session = conn.session.lock().await;
    if session.is_none() {
        let opened = conn
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(SESSION_TIMEOUT))
            .await
            .map_err(|e| explain_start_failure(harness, &e))?;
        *session = opened.get("sessionId").and_then(Value::as_str).map(str::to_owned);
    }
    session.clone().ok_or_else(|| "the agent opened no session".to_owned())
}

/// Stops the running prompt when this is dropped, so a cancelled turn doesn't keep the agent working.
struct PromptGuard(tokio::task::JoinHandle<Result<Value, String>>);

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Sends the prompt and streams the agent's work as events until it answers. Stopping through `cancel` asks the
/// agent to stop and waits for its answer.
pub(super) async fn run(
    conn: &Arc<Conn>,
    harness: &Harness,
    session: &str,
    prompt: &[PromptPart],
    events: &UnboundedSender<AcpEvent>,
    cancel: &CancellationToken,
) -> Result<Outcome, String> {
    let (tx, mut incoming) = mpsc::unbounded_channel();
    *conn.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);

    let (prompt_conn, session_id, parts) = (conn.clone(), session.to_owned(), prompt_json(prompt));
    let mut done = PromptGuard(tokio::spawn(async move {
        prompt_conn.request("session/prompt", json!({ "sessionId": session_id, "prompt": parts }), None).await
    }));
    let mut updates = Updates::default();
    let mut cancelled = false;

    let answer = loop {
        tokio::select! {
            result = &mut done.0 => break result.unwrap_or_else(|e| Err(e.to_string())),
            () = cancel.cancelled(), if !cancelled => {
                cancelled = true;
                let _ = conn.notify("session/cancel", json!({ "sessionId": session })).await;
            }
            Some(message) = incoming.recv() => match message {
                Incoming::Permission { id, params } => ask_permission(conn, harness, id, &params, events).await,
                Incoming::Update(params) => {
                    if let Some(update) = params.get("update") {
                        for event in updates.apply(update) {
                            let _ = events.send(event);
                        }
                    }
                }
            },
        }
    };
    *conn.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    for event in updates.stop_open() {
        let _ = events.send(event);
    }
    let result = answer.map_err(|e| format!("{}: {e}", harness.name))?;
    let stop = result.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn");
    Ok(match stop {
        "cancelled" => Outcome::Stopped,
        "refusal" => Outcome::Refused,
        "max_tokens" | "max_turn_requests" => Outcome::Limit(stop.to_owned()),
        _ => Outcome::Finished,
    })
}

/// Hands a permission request to whoever is watching and answers the agent with what they chose. No answer
/// is a refusal.
async fn ask_permission(conn: &Conn, harness: &Harness, id: Value, params: &Value, events: &UnboundedSender<AcpEvent>) {
    let options = params.get("options").and_then(Value::as_array).cloned().unwrap_or_default();
    let pick = |kinds: &[&str]| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str).is_some_and(|k| kinds.contains(&k)))
            .and_then(|o| o.get("optionId").cloned())
    };
    let call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let tool = ToolInfo {
        id: call.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_owned(),
        name: tool_name(call.get("kind").and_then(Value::as_str).unwrap_or_default()).to_owned(),
        args: call.get("rawInput").cloned().unwrap_or_else(|| json!({ "title": call.get("title") })),
    };
    let why = call.get("title").and_then(Value::as_str).map(|t| format!("{} wants to: {t}", harness.name));
    let (reply, answer) = oneshot::channel();
    let allowed = if events.send(AcpEvent::Permission(PermissionRequest { tool, why, reply })).is_ok() {
        answer.await.unwrap_or(false)
    } else {
        false
    };
    let option = if allowed { pick(&["allow_once", "allow_always"]) } else { pick(&["reject_once", "reject_always"]) };
    let outcome = match option {
        Some(option) => json!({ "outcome": { "outcome": "selected", "optionId": option } }),
        None => json!({ "outcome": { "outcome": "cancelled" } }),
    };
    if conn.reply(id, outcome).await.is_err() {
        trace(&harness.name, "couldn't answer a permission request: the agent is gone");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events_of(updates: &mut Updates, list: &[Value]) -> Vec<AcpEvent> {
        list.iter().flat_map(|u| updates.apply(u)).collect()
    }

    #[test]
    fn diffs_and_plans_read_as_the_chat_draws_them() {
        let (text, diff) = tool_content(Some(&json!([{ "type": "diff", "path": "a.rs", "oldText": "x", "newText": "y" }])));
        assert_eq!(text, "Edited a.rs");
        assert_eq!(diff, Some(("a.rs".into(), Some("x".into()), "y".into())));
        let plan = plan_text(&[json!({ "content": "Read", "status": "completed" }), json!({ "content": "Fix", "status": "in_progress" }), json!({ "content": "Test", "status": "pending" })]);
        assert_eq!(plan, "[x] Read\n[~] Fix\n[ ] Test");
        assert_eq!(tool_name("execute"), "bash");
    }

    #[test]
    fn a_tool_call_starts_once_and_ends_with_its_output() {
        let mut updates = Updates::default();
        let events = events_of(
            &mut updates,
            &[
                json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "execute", "title": "cargo test"}),
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress"}),
                json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"text": "ok"}}]}),
            ],
        );
        assert_eq!(events.len(), 2);
        assert!(matches!(&events[0], AcpEvent::ToolStart { name, args, .. } if name == "bash" && args["command"] == "cargo test"));
        assert!(matches!(&events[1], AcpEvent::ToolEnd { output, is_error: false, .. } if output == "ok"));
    }

    #[test]
    fn an_edit_with_a_diff_carries_before_and_after() {
        let mut updates = Updates::default();
        let events = events_of(
            &mut updates,
            &[json!({"sessionUpdate": "tool_call", "toolCallId": "t", "kind": "edit", "status": "completed",
                     "content": [{"type": "diff", "path": "a.rs", "oldText": "x", "newText": "y"}]})],
        );
        let Some(AcpEvent::ToolEnd { name, args, .. }) = events.last() else { panic!("no end") };
        assert_eq!((name.as_str(), args["old_string"].as_str(), args["new_string"].as_str()), ("edit", Some("x"), Some("y")));
    }

    #[test]
    fn calls_left_open_end_as_stopped() {
        let mut updates = Updates::default();
        events_of(&mut updates, &[json!({"sessionUpdate": "tool_call", "toolCallId": "t", "kind": "read"})]);
        let ended = updates.stop_open();
        assert!(matches!(&ended[..], [AcpEvent::ToolEnd { output, is_error: true, .. }] if output == "Stopped."));
        assert!(updates.stop_open().is_empty());
    }

    #[test]
    fn text_and_thoughts_pass_through_and_unknown_updates_are_ignored() {
        let mut updates = Updates::default();
        let events = events_of(
            &mut updates,
            &[
                json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "hi"}}),
                json!({"sessionUpdate": "agent_thought_chunk", "content": {"text": "hm"}}),
                json!({"sessionUpdate": "available_commands_update"}),
            ],
        );
        assert!(matches!(&events[..], [AcpEvent::Text(a), AcpEvent::Thought(b)] if a == "hi" && b == "hm"));
    }

    #[test]
    fn a_missing_sign_in_is_explained_and_other_failures_are_plain() {
        let harness = Harness {
            id: "codex-acp".into(),
            name: "Codex".into(),
            description: String::new(),
            icon: None,
            available: true,
            ready: false,
            launcher: "npx".into(),
            args: vec![],
            env: HashMap::new(),
        };
        assert!(explain_start_failure(&harness, "authentication required").contains("codex login"));
        assert!(explain_start_failure(&harness, "boom").contains("couldn't start a session"));
    }
}
