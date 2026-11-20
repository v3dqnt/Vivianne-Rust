//! One turn with an agent: send the user's message, turn what the agent streams back into [`AcpEvent`]s, and
//! end the turn the way the protocol says it ends.
//!
//! A v1 turn ends on the answer to `session/prompt`. A v2 turn ends when the agent has taken the prompt, nothing
//! is waiting on the user, and the session says it is idle: the answer only says the prompt was inserted, and the
//! work it started goes on after it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use vivianne_ai::ai::types::{CostBreakdown, Usage};

use super::conn::{Conn, Incoming, RpcError, kill_tree, trace};
use super::info::{apply_session_info, remember_context};
use super::permission::{Disposition, call_id, cancelled_outcome, disposition, permission_ask, permission_outcome, tool_name};
use super::registry::Harness;
use super::session::{DELEGATE, Line, digest, explain_start_failure, forget_session, looks_unauthenticated, save_session, session_key};
use super::stderr;
use crate::Spent;

/// How long `session/close` gets to answer after a session-wide Stop.
const STOP_CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a turn rechecks whether it has settled (a dialog that closed after the session went idle).
const SETTLE_CHECK: Duration = Duration::from_millis(100);

/// What the agent is doing, as the chat can draw it. The agent runs its own tools, so tool calls here are
/// things that happen, not things to run.
#[derive(Debug)]
pub enum AcpEvent {
    Text(String),
    Thought(String),
    ToolStart { id: String, name: String, args: Value },
    ToolEnd { id: String, name: String, args: Value, output: String, is_error: bool },
    /// The agent's plan, as a checklist (`[x]` done, `[~]` doing, `[ ]` to do). Empty when the plan was removed.
    Plan(String),
    /// Something the agent wants the user to see that is not part of the work.
    Notice { title: String, text: String, is_error: bool },
    /// The agent wants to do something and asks first. It waits until this is answered.
    Permission(PermissionRequest),
    /// What the turn used, sent once as it ends, when the agent said. Whoever knows the thread records it.
    Usage(Spent),
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
    /// It did not finish, and why.
    Failed(String),
}

/// A piece of what the user sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptPart {
    Text(String),
    Image { base64: String, mime: String },
}

/// What a turn needs to know.
#[derive(Debug)]
pub struct TurnRequest<'a> {
    /// The user's message.
    pub prompt: &'a [PromptPart],
    /// The conversation before this message, for an agent that wasn't there for it.
    pub earlier: &'a [Line],
    /// Plan mode: the agent may look but not change anything.
    pub plan_mode: bool,
    /// Bypass mode: every request is allowed without asking.
    pub bypass: bool,
    /// The user's MCP servers, handed to the agent when its session opens.
    pub mcp_servers: &'a [super::own_tools::McpServerSpec],
    /// Vivianne's own tools, offered to the agent for this turn.
    pub own: Option<super::own_tools::OwnTools>,
}

/// What a turn produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub outcome: Outcome,
    /// What the agent said, for the conversation's record.
    pub reply: String,
}

// ── Reading the agent's updates ─────────────────────────────────────────────

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

/// A change to the plan an agent keeps.
#[derive(Debug, PartialEq, Eq)]
enum PlanChange {
    Show { id: String, text: String },
    Remove { id: String },
}

/// `plan` (v1: the whole list every time), `plan_update` (v2: a list, a markdown text, or a file, under a plan
/// id) and `plan_removed`.
fn plan_change(kind: &str, u: &Value) -> Option<PlanChange> {
    match kind {
        "plan" => Some(PlanChange::Show {
            id: "legacy".to_owned(),
            text: plan_text(u.get("entries").and_then(Value::as_array).map_or(&[][..], Vec::as_slice)),
        }),
        "plan_update" => {
            let plan = u.get("plan")?;
            let id = plan.get("planId")?.as_str()?.trim();
            if id.is_empty() {
                return None;
            }
            let text = match plan.get("type")?.as_str()? {
                "items" => plan_text(plan.get("entries")?.as_array()?),
                "markdown" => plan.get("content")?.as_str()?.to_owned(),
                "file" => format!("Plan: {}", plan.get("uri")?.as_str()?),
                // A kind of plan this client has never heard of: nothing honest to draw.
                _ => return None,
            };
            Some(PlanChange::Show { id: id.to_owned(), text })
        }
        "plan_removed" => {
            let id = u.get("planId")?.as_str()?.trim();
            (!id.is_empty()).then(|| PlanChange::Remove { id: id.to_owned() })
        }
        _ => None,
    }
}

/// The text of a content block, when it is text.
fn block_text(block: Option<&Value>) -> Option<String> {
    let b = block?;
    (b.get("type").and_then(Value::as_str) == Some("text")).then(|| b.get("text").and_then(Value::as_str).map(str::to_owned)).flatten()
}

fn blocks_text(blocks: &[Value]) -> String {
    blocks.iter().filter_map(|b| block_text(Some(b))).collect::<Vec<_>>().join("\n")
}

/// What a `notice` says, bounded, or `None` when it has no title to show: (title, description, severity).
fn notice(u: &Value) -> Option<(String, Option<String>, String)> {
    let cut = |key: &str, max: usize| u.get(key).and_then(Value::as_str).map(|s| s.trim().chars().take(max).collect::<String>()).filter(|s| !s.is_empty());
    let title = cut("title", 1024)?;
    Some((title, cut("description", 4096), cut("severity", 64).unwrap_or_else(|| "info".to_owned())))
}

/// Updates this client knows about but has nothing to draw for yet. Anything else is worth a trace line, once,
/// so a new protocol feature is not silently dropped.
fn known_update(kind: &str) -> bool {
    matches!(
        kind,
        "user_message_chunk" | "user_message" | "agent_message" | "agent_thought" | "tool_call_content_chunk" | "terminal_update" | "terminal_output_chunk"
    )
}

/// A tool call being built up from `tool_call` and `tool_call_update` messages.
struct Call {
    name: &'static str,
    args: Value,
    started: bool,
    /// The agent's own word for what it is (`read`, `edit`, `execute`...), kept because a permission request for
    /// the call may leave it out.
    kind: String,
}

/// Turns the agent's updates into events, remembering what is in flight and what the agent said about its state.
struct Updates {
    who: String,
    thread: String,
    follows_work: bool,
    calls: HashMap<String, Call>,
    /// The session's work state (v2): `running`, `idle`, `requires_action`.
    work: Option<String>,
    idle_reason: Option<String>,
    idle_error: Option<Value>,
    cost: Option<f64>,
    shown_plan: Option<String>,
    compactions: HashMap<String, String>,
    unknown: HashSet<String>,
}

impl Updates {
    fn new(who: &str, thread: &str, follows_work: bool) -> Self {
        Self {
            who: who.to_owned(),
            thread: thread.to_owned(),
            follows_work,
            calls: HashMap::new(),
            work: None,
            idle_reason: None,
            idle_error: None,
            cost: None,
            shown_plan: None,
            compactions: HashMap::new(),
            unknown: HashSet::new(),
        }
    }

    /// The agent's word for the kind of a tool call it announced.
    fn kind_of(&self, id: &str) -> Option<String> {
        self.calls.get(id).map(|c| c.kind.clone()).filter(|k| !k.is_empty())
    }

    fn apply(&mut self, conn: &Conn, update: &Value) -> Vec<AcpEvent> {
        let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default();
        match kind {
            "agent_message_chunk" => text_chunk(update).map(AcpEvent::Text).into_iter().collect(),
            "agent_thought_chunk" => text_chunk(update).map(AcpEvent::Thought).into_iter().collect(),
            "state_update" => {
                self.state(update);
                Vec::new()
            }
            "tool_call" | "tool_call_update" => self.tool(update),
            "plan" | "plan_update" | "plan_removed" => self.plan(kind, update),
            "compaction_update" | "compaction_summary_chunk" => self.compaction(kind, update),
            "notice" => notice(update)
                .map(|(title, description, severity)| AcpEvent::Notice { text: description.unwrap_or_else(|| title.clone()), title, is_error: severity == "error" })
                .into_iter()
                .collect(),
            "available_commands_update" | "current_mode_update" | "config_option_update" | "session_info_update" => {
                apply_session_info(&mut conn.info.lock().unwrap_or_else(std::sync::PoisonError::into_inner), kind, update);
                Vec::new()
            }
            "usage_update" => {
                if let (Some(used), Some(size)) = (update.get("used").and_then(Value::as_u64), update.get("size").and_then(Value::as_u64)) {
                    remember_context(&self.thread, used, size);
                }
                if let Some(amount) = update.pointer("/cost/amount").and_then(Value::as_f64) {
                    self.cost = Some(amount);
                }
                Vec::new()
            }
            other => {
                if !known_update(other) && self.unknown.insert(other.to_owned()) {
                    trace(&self.who, format!("session update this client does not know: {other}"));
                }
                Vec::new()
            }
        }
    }

    /// A v2 session's work state. v1 has no such update.
    fn state(&mut self, u: &Value) {
        if !self.follows_work {
            return;
        }
        let Some(now) = u.get("state").and_then(Value::as_str) else { return };
        if !matches!(now, "running" | "idle" | "requires_action") && self.work.as_deref() != Some(now) {
            // A state this client doesn't know is never read as idle: it may mean anything, including more work.
            trace(&self.who, format!("state update this client does not know: {now}"));
        }
        self.work = Some(now.to_owned());
        if now == "idle" {
            // An idle may say why the work stopped, or say nothing: no reason is a turn that finished.
            self.idle_reason = Some(u.get("stopReason").and_then(Value::as_str).unwrap_or("end_turn").to_owned());
            self.idle_error = u.get("error").filter(|e| e.is_object()).cloned();
        }
    }

    fn tool(&mut self, u: &Value) -> Vec<AcpEvent> {
        let Some(id) = u.get("toolCallId").and_then(Value::as_str).map(str::to_owned) else { return Vec::new() };
        let call = self.calls.entry(id.clone()).or_insert_with(|| Call { name: "tool", args: json!({}), started: false, kind: String::new() });
        if let Some(kind) = u.get("kind").and_then(Value::as_str) {
            call.name = tool_name(kind);
            kind.clone_into(&mut call.kind);
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
        let call = self.calls.remove(id).unwrap_or(Call { name: "tool", args: json!({}), started: true, kind: String::new() });
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

    fn plan(&mut self, kind: &str, u: &Value) -> Vec<AcpEvent> {
        let Some(change) = plan_change(kind, u) else {
            if self.unknown.insert(format!("{kind}:unreadable")) {
                trace(&self.who, format!("a {kind} this client can't read: {u}"));
            }
            return Vec::new();
        };
        match change {
            PlanChange::Show { id, text } => {
                self.shown_plan = Some(id);
                vec![AcpEvent::Plan(text)]
            }
            // Removing the plan on show empties the panel; removing one that was never the one on show changes nothing.
            PlanChange::Remove { id } if self.shown_plan.as_deref() == Some(id.as_str()) => {
                self.shown_plan = None;
                vec![AcpEvent::Plan(String::new())]
            }
            PlanChange::Remove { .. } => Vec::new(),
        }
    }

    /// The agent compacting its own context: one row, open while it runs.
    fn compaction(&mut self, kind: &str, u: &Value) -> Vec<AcpEvent> {
        let Some(compaction) = u.get("compactionId").and_then(Value::as_str) else { return Vec::new() };
        let id = format!("acp-compaction:{compaction}");
        let args = json!({ "title": "Compact context" });
        let mut events = Vec::new();
        if !self.compactions.contains_key(&id) {
            self.compactions.insert(id.clone(), String::new());
            events.push(AcpEvent::ToolStart { id: id.clone(), name: "compact_context".to_owned(), args: args.clone() });
        }
        if kind == "compaction_summary_chunk" {
            if let (Some(text), Some(so_far)) = (block_text(u.get("content")), self.compactions.get_mut(&id)) {
                so_far.push_str(&text);
            }
            return events;
        }
        let summary = u.get("summary").and_then(Value::as_array).map(|b| blocks_text(b)).unwrap_or_default();
        match u.get("status").and_then(Value::as_str).unwrap_or("in_progress") {
            done @ ("completed" | "failed" | "cancelled") => {
                let streamed = self.compactions.remove(&id).unwrap_or_default();
                let failed = done != "completed";
                let output = if failed {
                    u.get("error").and_then(Value::as_str).filter(|e| !e.is_empty()).map_or_else(|| format!("Compaction {done}."), str::to_owned)
                } else if !summary.is_empty() {
                    summary
                } else if !streamed.is_empty() {
                    streamed
                } else {
                    "Done.".to_owned()
                };
                events.push(AcpEvent::ToolEnd { id, name: "compact_context".to_owned(), args, output, is_error: failed });
            }
            // Still going, or a state this client doesn't know: still going.
            _ => {
                if !summary.is_empty() {
                    self.compactions.insert(id, summary);
                }
            }
        }
        events
    }

    /// Ends the calls and compactions a stop or a failure left open.
    fn stop_open(&mut self) -> Vec<AcpEvent> {
        let mut events: Vec<AcpEvent> = self
            .compactions
            .drain()
            .map(|(id, _)| AcpEvent::ToolEnd { id, name: "compact_context".to_owned(), args: json!({ "title": "Compact context" }), output: "Stopped.".to_owned(), is_error: true })
            .collect();
        events.extend(self.calls.drain().filter(|(_, call)| call.started).map(|(id, call)| AcpEvent::ToolEnd {
            id,
            name: call.name.to_owned(),
            args: call.args,
            output: "Stopped.".to_owned(),
            is_error: true,
        }));
        events
    }
}

fn text_chunk(update: &Value) -> Option<String> {
    update.pointer("/content/text").and_then(Value::as_str).map(str::to_owned)
}

/// A v2 turn is over when the agent has taken the prompt (or the user has stopped it), nothing is waiting on
/// the user, and the session says it is idle.
fn turn_settled(work: Option<&str>, begun: bool, waiting: usize) -> bool {
    begun && waiting == 0 && work == Some("idle")
}

fn prompt_json(parts: &[PromptPart], images_ok: bool, who: &str) -> Vec<Value> {
    parts
        .iter()
        .map(|part| match part {
            PromptPart::Text(text) => json!({ "type": "text", "text": text }),
            PromptPart::Image { .. } if !images_ok => json!({ "type": "text", "text": format!("(The user attached an image, which {who} can't read.)") }),
            PromptPart::Image { base64, mime } => json!({ "type": "image", "data": base64, "mimeType": mime }),
        })
        .collect()
}

/// A turn's tokens from the answer to `session/prompt`, and its cost when the agent reported one. `None` when
/// it reported neither.
fn turn_usage(answer: &Value, cost: Option<f64>) -> Option<Usage> {
    let tokens = answer.get("usage");
    if tokens.is_none() && cost.is_none() {
        return None;
    }
    let n = |key: &str| tokens.and_then(|u| u.get(key)).and_then(Value::as_u64).map_or(0, |v| u32::try_from(v).unwrap_or(u32::MAX));
    let (input, output, total) = (n("inputTokens"), n("outputTokens"), n("totalTokens"));
    Some(Usage {
        input,
        output,
        cache_read: n("cachedReadTokens"),
        cache_write: n("cachedWriteTokens"),
        total_tokens: if total > 0 { total } else { input + output },
        cost: cost.map(|total| CostBreakdown { total, ..CostBreakdown::default() }),
    })
}

// ── Stopping ────────────────────────────────────────────────────────────────

/// Stops a turn for good. `session/cancel` is a notification sent from its own task, so an agent that has
/// stopped reading cannot hold Stop up. A session-wide Stop also closes the session, which is what ends the
/// work a cancel alone leaves running: every v2 agent has `session/close`, a v1 agent only when it says so.
/// Returns whether the session was closed, so the turn is not saved for a session that no longer exists.
async fn stop_turn(conn: &Arc<Conn>, session: &str, thread: &str, harness_id: &str) -> bool {
    let cancel = conn.clone();
    let id = session.to_owned();
    tokio::spawn(async move {
        let _ = cancel.notify("session/cancel", json!({ "sessionId": id })).await;
    });
    if !conn.cap("/capabilities/session", "/agentCapabilities/sessionCapabilities/close") {
        return false;
    }
    // The session is gone from here on: a turn that starts before the close is answered must open a new one
    // rather than reuse it.
    *conn.session.lock().await = None;
    let close = conn.clone();
    let (id, key, saved) = (session.to_owned(), session_key(thread, harness_id), !thread.starts_with(DELEGATE));
    tokio::spawn(async move {
        // A close refused because the session is already gone is a close; one that hangs is abandoned, and the
        // kill fallback covers it.
        let _ = close.request("session/close", json!({ "sessionId": id }), Some(STOP_CLOSE_TIMEOUT)).await;
        if saved {
            forget_session(&key, &id);
        }
    });
    true
}

// ── What the agent asks ─────────────────────────────────────────────────────

/// Answers one permission request: allows it, refuses it, or asks the user, and gives up on the user when Stop
/// is pressed. Runs on its own task so the turn's other updates keep flowing while a dialog is open.
#[allow(clippy::too_many_arguments)]
async fn answer_permission(
    conn: Arc<Conn>,
    who: String,
    events: UnboundedSender<AcpEvent>,
    cancel: CancellationToken,
    id: Value,
    params: Value,
    known_kind: Option<String>,
    (plan_mode, bypass): (bool, bool),
) {
    let ask = permission_ask(&params, known_kind);
    let outcome = match disposition(ask.kind.as_deref().unwrap_or("other"), plan_mode, bypass) {
        Disposition::Allow => permission_outcome(&ask.options, true),
        Disposition::Deny => {
            trace(&who, format!("refused in plan mode: {}", ask.title.as_deref().unwrap_or("?")));
            permission_outcome(&ask.options, false)
        }
        Disposition::Ask => {
            let (reply, answer) = oneshot::channel();
            let why = ask.title.as_deref().map(|t| format!("{who} wants to: {t}"));
            let request = PermissionRequest { tool: ToolInfo { id: ask.id.clone(), name: ask.name.clone(), args: ask.args.clone() }, why, reply };
            if events.send(AcpEvent::Permission(request)).is_err() {
                permission_outcome(&ask.options, false)
            } else {
                tokio::select! {
                    // Stopped: the protocol asks a client that cancels to answer every open request this way.
                    () = cancel.cancelled() => cancelled_outcome(),
                    decided = answer => permission_outcome(&ask.options, decided.unwrap_or(false)),
                }
            }
        }
    };
    let _ = conn.reply(id, outcome).await;
}

// ── The turn ────────────────────────────────────────────────────────────────

/// What the agent hasn't been told about the thread, then the user's message.
fn build_prompt(conn: &Conn, harness: &Harness, request: &TurnRequest<'_>) -> Vec<Value> {
    let mut prompt = Vec::new();
    // A message that starts with `/` is a command of the agent's own: nothing goes before it, or it would no
    // longer start the prompt.
    let slash = request.prompt.iter().find_map(|p| if let PromptPart::Text(text) = p { Some(text.trim_start().starts_with('/')) } else { None }) == Some(true);
    if !slash {
        let seen = conn.seen.load(Ordering::SeqCst);
        let missed = request.earlier.get(seen..).unwrap_or(request.earlier);
        if let Some(digest) = digest(missed) {
            prompt.push(json!({ "type": "text", "text": digest }));
        }
        // Where it is running and what it has: said once, and again only when that changes. Only to a turn that
        // is for the user (one that carries Vivianne's tools), not to an internal check such as a sign-in probe.
        let tools: &[super::own_tools::OfferedTool] = request.own.as_ref().map_or(&[], |own| &own.tools);
        let told_now = if request.own.is_none() { None } else {
            let mut last = conn.told.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let (send, now) = super::own_tools::next_instructions(&last, request.plan_mode, tools);
            *last = now;
            send
        };
        if let Some(text) = told_now {
            prompt.push(json!({ "type": "text", "text": text }));
        }
    }
    prompt.extend(prompt_json(request.prompt, conn.takes_images(), &harness.name));
    prompt
}

/// Ends the turn's access to Vivianne's tools when it is dropped, however the turn ends.
struct EndOwnTools(Arc<Conn>);

impl Drop for EndOwnTools {
    fn drop(&mut self) {
        self.0.mcp.end();
    }
}

/// Stops the running prompt when this is dropped, so a cancelled turn doesn't keep the agent working.
struct PromptGuard(tokio::task::JoinHandle<Result<Value, RpcError>>);

impl Drop for PromptGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Runs the turn on an open session, sending what the agent does into `events`.
#[allow(clippy::too_many_lines)]
pub(super) async fn run(
    conn: &Arc<Conn>,
    harness: &Harness,
    session: &str,
    thread: &str,
    request: &TurnRequest<'_>,
    events: &UnboundedSender<AcpEvent>,
    cancel: &CancellationToken,
) -> TurnEnd {
    let prompt = build_prompt(conn, harness, request);
    // Vivianne's own tools answer calls for this turn, and only this one.
    let _own = request.own.as_ref().map(|own| {
        conn.mcp.begin(own.call.clone());
        EndOwnTools(conn.clone())
    });
    let (tx, mut incoming) = mpsc::unbounded_channel();
    *conn.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tx);
    let (prompt_conn, session_id) = (conn.clone(), session.to_owned());
    let mut done = PromptGuard(tokio::spawn(async move {
        prompt_conn.request("session/prompt", json!({ "sessionId": session_id, "prompt": prompt }), None).await
    }));

    let follows_work = conn.speaks_v2();
    let mut updates = Updates::new(&harness.name, thread, follows_work);
    let mut reply = String::new();
    let mut response = Value::Null;
    // A v2 turn ends on the session going idle, not on the answer to `session/prompt`.
    let mut accepted = !follows_work;
    let mut answered = false;
    // Dialogs waiting on the user: a turn does not end under one.
    let waiting = Arc::new(AtomicUsize::new(0));
    let mut stopped_at: Option<Instant> = None;
    let mut closed = false;
    let mut settle = tokio::time::interval(SETTLE_CHECK);

    let answer = loop {
        let kill_at = stopped_at.map(|at| at + Duration::from_millis(conn.stop_kill_after.load(Ordering::SeqCst)));
        tokio::select! {
            result = &mut done.0, if !answered => {
                answered = true;
                match result.unwrap_or_else(|e| Err(RpcError::transport(e.to_string()))) {
                    Ok(value) => {
                        accepted = true;
                        response = value;
                        // v1: the answer is the end of the turn. v2: unless the session already went idle, its
                        // work is still running.
                        if !follows_work || turn_settled(updates.work.as_deref(), true, waiting.load(Ordering::SeqCst)) {
                            break Ok(response.clone());
                        }
                    }
                    Err(e) => break Err(e),
                }
            }
            () = cancel.cancelled(), if stopped_at.is_none() => {
                stopped_at = Some(Instant::now());
                closed = stop_turn(conn, session, thread, &harness.id).await;
            }
            // An agent that ignores `session/cancel` would leave the turn open forever: end its process tree,
            // and end the turn here.
            () = async { match kill_at { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {
                if let Some(pid) = conn.pid() {
                    kill_tree(pid).await;
                }
                // Its session died with it, so the next turn starts a new one.
                *conn.session.lock().await = None;
                closed = true;
                if !thread.starts_with(DELEGATE) {
                    forget_session(&session_key(thread, &harness.id), session);
                }
                conn.alive.store(false, Ordering::SeqCst);
                break Ok(json!({ "stopReason": "cancelled" }));
            }
            // A dialog that closed after the session went idle ends the turn too: the last update may have
            // arrived under the dialog.
            _ = settle.tick(), if follows_work => {
                if turn_settled(updates.work.as_deref(), accepted || stopped_at.is_some(), waiting.load(Ordering::SeqCst)) {
                    break Ok(response.clone());
                }
            }
            Some(message) = incoming.recv() => match message {
                // Each is answered on a task of its own: a dialog waiting on the user must not hold up the
                // agent's other updates, or Stop.
                Incoming::Permission { id, params } => {
                    let known_kind = call_id(&params).and_then(|call| updates.kind_of(call));
                    waiting.fetch_add(1, Ordering::SeqCst);
                    let (conn, events, cancel, waiting) = (conn.clone(), events.clone(), cancel.clone(), waiting.clone());
                    let who = harness.name.clone();
                    let modes = (request.plan_mode, request.bypass);
                    tokio::spawn(async move {
                        answer_permission(conn, who, events, cancel, id, params, known_kind, modes).await;
                        waiting.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                // A form: Vivianne has nowhere to show one, so it is declined and the agent goes on without it.
                Incoming::Elicitation { id } => {
                    trace(&harness.name, "declined a form it can't show");
                    let conn = conn.clone();
                    tokio::spawn(async move {
                        let _ = conn.reply(id, json!({ "action": "decline" })).await;
                    });
                }
                Incoming::Update(params) => {
                    let Some(update) = params.get("update") else { continue };
                    for event in updates.apply(conn, update) {
                        if let AcpEvent::Text(piece) = &event {
                            reply.push_str(piece);
                        }
                        let _ = events.send(event);
                    }
                    if follows_work && turn_settled(updates.work.as_deref(), accepted || stopped_at.is_some(), waiting.load(Ordering::SeqCst)) {
                        break Ok(response.clone());
                    }
                }
            },
        }
    };
    *conn.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    for event in updates.stop_open() {
        let _ = events.send(event);
    }

    let outcome = outcome_of(conn, harness, &answer, &updates, stopped_at.is_some()).await;
    // This turn's tokens and cost, reported for whoever knows the thread to record.
    if let Ok(result) = &answer
        && let Some(spent) = turn_spent(conn, harness, result, updates.cost)
    {
        let _ = events.send(AcpEvent::Usage(spent));
    }
    // What it has seen of the thread now: everything up to and including this exchange.
    let seen = request.earlier.len() + 1 + usize::from(!reply.is_empty());
    conn.seen.store(seen, Ordering::SeqCst);
    if !closed && !thread.starts_with(DELEGATE) {
        save_session(&session_key(thread, &harness.id), session, seen);
    }
    TurnEnd { outcome, reply }
}

/// What the end of the turn means: finished, stopped, or failed and why.
async fn outcome_of(conn: &Arc<Conn>, harness: &Harness, answer: &Result<Value, RpcError>, updates: &Updates, stopped: bool) -> Outcome {
    match answer {
        Ok(result) => {
            // A v2 turn's reason comes from the idle state that ended it; a v1 turn's from the answer to
            // `session/prompt`.
            let stop = updates.idle_reason.as_deref().or_else(|| result.get("stopReason").and_then(Value::as_str)).unwrap_or("end_turn");
            match stop {
                "refusal" => Outcome::Failed(format!("{} declined to continue.", harness.name)),
                "max_tokens" | "max_turn_requests" => Outcome::Failed(format!("{} stopped: it hit its limit ({stop}).", harness.name)),
                // After a Stop the turn is the user's, not a failure.
                "error" if !stopped => {
                    let why = updates.idle_error.as_ref().and_then(|e| e.get("message")).and_then(Value::as_str).unwrap_or_default();
                    let said = if why.is_empty() {
                        format!("{} stopped: something went wrong.", harness.name)
                    } else if looks_unauthenticated(why) {
                        explain_start_failure(harness, why)
                    } else {
                        format!("{}: {why}", harness.name)
                    };
                    Outcome::Failed(with_stderr(conn, said).await)
                }
                "cancelled" | "error" => Outcome::Stopped,
                _ => Outcome::Finished,
            }
        }
        Err(e) => {
            if stopped {
                // A turn that fails after Stop was stopped, not broken: the process usually dies because Stop
                // ended it, and saying so as an error would blame the user for their own Stop.
                trace(&harness.name, format!("the stopped turn's answer failed: {e}"));
                return Outcome::Stopped;
            }
            // A turn that fails because the agent isn't signed in says how to fix it.
            let said = if e.is_auth_required() || looks_unauthenticated(&e.message) {
                explain_start_failure(harness, &e.message)
            } else {
                format!("{}: {e}", harness.name)
            };
            Outcome::Failed(with_stderr(conn, said).await)
        }
    }
}

/// What the agent said on the way out explains a failure it didn't name.
async fn with_stderr(conn: &Conn, mut said: String) -> String {
    if !conn.is_alive() {
        let tail = stderr::excerpt(conn).await;
        if !tail.is_empty() {
            said.push_str("\n\n");
            said.push_str(&tail);
        }
    }
    said
}

/// This turn's share of the session's cost, and its tokens, as what the agent spent. `None` when it reported
/// neither. The cost is `None` when the agent gave no figure: unknown, not zero.
fn turn_spent(conn: &Conn, harness: &Harness, result: &Value, cost_now: Option<f64>) -> Option<Spent> {
    let share = cost_now.map(|now| {
        let mut last = conn.cost.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let share = (now - *last).max(0.0);
        *last = now;
        share
    });
    let usage = turn_usage(result, share)?;
    Some(Spent {
        provider: super::PROVIDER.to_owned(),
        model: harness.id.clone(),
        input: u64::from(usage.input),
        output: u64::from(usage.output),
        cache_read: u64::from(usage.cache_read),
        cache_write: u64::from(usage.cache_write),
        cost: usage.cost.map(|c| c.total),
    })
}

#[cfg(test)]
mod tests {
    use super::super::conn::testing::{Peer, harness, peer};
    use super::*;

    const SESSION: &str = "s1";

    fn thread() -> String {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        format!("turn-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst))
    }

    /// A connection that has finished `initialize` as an agent of `version` with `caps`.
    async fn ready(version: u64, caps: Value) -> (Arc<Conn>, Peer) {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { super::super::conn::handshake(&conn, &harness("a")).await }
        });
        agent.hello_version(version, caps, json!([])).await;
        assert!(asked.await.is_ok_and(|r| r.is_ok()));
        (conn, agent)
    }

    struct Started {
        turn: tokio::task::JoinHandle<TurnEnd>,
        events: mpsc::UnboundedReceiver<AcpEvent>,
        cancel: CancellationToken,
    }

    fn start(conn: &Arc<Conn>, text: &str, plan_mode: bool, bypass: bool) -> Started {
        let (tx, events) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();
        let (conn, thread, stop, who) = (conn.clone(), thread(), cancel.clone(), harness("a"));
        let prompt = vec![PromptPart::Text(text.to_owned())];
        let turn = tokio::spawn(async move {
            let request = TurnRequest { prompt: &prompt, earlier: &[], plan_mode, bypass, mcp_servers: &[], own: None };
            let end = run(&conn, &who, SESSION, &thread, &request, &tx, &stop).await;
            forget_session(&session_key(&thread, "a"), SESSION);
            end
        });
        Started { turn, events, cancel }
    }

    async fn collect(mut started: Started) -> (TurnEnd, Vec<AcpEvent>) {
        let end = started.turn.await.unwrap_or(TurnEnd { outcome: Outcome::Failed("task".into()), reply: String::new() });
        let mut events = Vec::new();
        while let Ok(event) = started.events.try_recv() {
            events.push(event);
        }
        (end, events)
    }

    async fn finish_v1(agent: &mut Peer, prompt: &Value, reason: &str) {
        agent.answer(&prompt["id"], json!({ "stopReason": reason })).await;
    }

    #[tokio::test]
    async fn a_reply_streams_and_the_v1_answer_ends_the_turn() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let started = start(&conn, "hi", false, false);
        let prompt = agent.prompt().await;
        assert_eq!(prompt["params"]["prompt"][0]["text"], "hi");
        agent.update(SESSION, json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "Hel"}})).await;
        agent.update(SESSION, json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "lo"}})).await;
        finish_v1(&mut agent, &prompt, "end_turn").await;
        let (end, events) = collect(started).await;
        assert_eq!(end, TurnEnd { outcome: Outcome::Finished, reply: "Hello".into() });
        assert!(matches!(&events[..], [AcpEvent::Text(a), AcpEvent::Text(b)] if a == "Hel" && b == "lo"));
    }

    #[tokio::test]
    async fn a_v2_turn_ends_when_the_session_goes_idle_not_when_the_prompt_is_taken() {
        let (conn, mut agent) = ready(2, json!({})).await;
        let started = start(&conn, "go", false, false);
        let prompt = agent.prompt().await;
        agent.answer(&prompt["id"], json!({})).await;
        agent.update(SESSION, json!({"sessionUpdate": "state_update", "state": "running"})).await;
        agent.update(SESSION, json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "working"}})).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!started.turn.is_finished(), "taking the prompt is not the end of the turn");
        agent.update(SESSION, json!({"sessionUpdate": "state_update", "state": "idle"})).await;
        let (end, events) = collect(started).await;
        assert_eq!(end.outcome, Outcome::Finished);
        assert!(matches!(&events[..], [AcpEvent::Text(t)] if t == "working"));
    }

    #[tokio::test]
    async fn a_v2_idle_that_names_an_error_fails_the_turn_with_its_message() {
        let (conn, mut agent) = ready(2, json!({})).await;
        let started = start(&conn, "go", false, false);
        let prompt = agent.prompt().await;
        agent.answer(&prompt["id"], json!({})).await;
        agent.update(SESSION, json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": "error", "error": {"message": "quota used up"}})).await;
        let (end, _) = collect(started).await;
        assert_eq!(end.outcome, Outcome::Failed("Mock: quota used up".into()));
    }

    #[tokio::test]
    async fn a_tool_call_becomes_a_start_and_an_end_and_an_edit_carries_its_diff() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let started = start(&conn, "edit", false, false);
        let prompt = agent.prompt().await;
        agent.update(SESSION, json!({"sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "execute", "title": "cargo test"})).await;
        agent.update(SESSION, json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}]})).await;
        agent.update(SESSION, json!({"sessionUpdate": "tool_call", "toolCallId": "t2", "kind": "edit", "status": "completed", "content": [{"type": "diff", "path": "a.rs", "oldText": "x", "newText": "y"}]})).await;
        finish_v1(&mut agent, &prompt, "end_turn").await;
        let (_, events) = collect(started).await;
        assert!(matches!(&events[0], AcpEvent::ToolStart { name, args, .. } if name == "bash" && args["command"] == "cargo test"));
        assert!(matches!(&events[1], AcpEvent::ToolEnd { output, is_error: false, .. } if output == "ok"));
        assert!(matches!(&events[3], AcpEvent::ToolEnd { name, args, .. } if name == "edit" && args["old_string"] == "x" && args["new_string"] == "y"));
    }

    #[tokio::test]
    async fn the_plan_is_shown_and_removing_it_empties_it() {
        let (conn, mut agent) = ready(2, json!({})).await;
        let started = start(&conn, "plan", false, false);
        let prompt = agent.prompt().await;
        agent.answer(&prompt["id"], json!({})).await;
        agent.update(SESSION, json!({"sessionUpdate": "plan_update", "plan": {"planId": "p1", "type": "items", "entries": [{"content": "Read", "status": "completed"}]}})).await;
        agent.update(SESSION, json!({"sessionUpdate": "plan_removed", "planId": "other"})).await;
        agent.update(SESSION, json!({"sessionUpdate": "plan_removed", "planId": "p1"})).await;
        agent.update(SESSION, json!({"sessionUpdate": "state_update", "state": "idle"})).await;
        let (_, events) = collect(started).await;
        assert!(matches!(&events[..], [AcpEvent::Plan(a), AcpEvent::Plan(b)] if a == "[x] Read" && b.is_empty()));
    }

    #[tokio::test]
    async fn a_permission_goes_to_the_user_and_grants_once() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let mut started = start(&conn, "write", false, false);
        let prompt = agent.prompt().await;
        let options = json!([{"kind": "allow_always", "optionId": "always"}, {"kind": "allow_once", "optionId": "once"}, {"kind": "reject_once", "optionId": "no"}]);
        agent.send(json!({"jsonrpc": "2.0", "id": 50, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "t", "kind": "edit", "title": "Edit a.rs"}, "options": options}})).await;
        let Some(AcpEvent::Permission(request)) = started.events.recv().await else { panic!("no request") };
        assert_eq!((request.tool.name.as_str(), request.why.as_deref()), ("edit", Some("Mock wants to: Edit a.rs")));
        request.answer(true);
        let answer = agent.recv().await;
        assert_eq!((answer["id"].as_u64(), answer["result"]["outcome"]["optionId"].as_str()), (Some(50), Some("once")));
        finish_v1(&mut agent, &prompt, "end_turn").await;
        assert_eq!(collect(started).await.0.outcome, Outcome::Finished);
    }

    #[tokio::test]
    async fn refusing_picks_the_reject_option() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let mut started = start(&conn, "write", false, false);
        let prompt = agent.prompt().await;
        let options = json!([{"kind": "allow_once", "optionId": "once"}, {"kind": "reject_once", "optionId": "no"}]);
        agent.send(json!({"jsonrpc": "2.0", "id": 51, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "t", "kind": "execute"}, "options": options}})).await;
        let Some(AcpEvent::Permission(request)) = started.events.recv().await else { panic!("no request") };
        request.answer(false);
        assert_eq!(agent.recv().await["result"]["outcome"]["optionId"], "no");
        finish_v1(&mut agent, &prompt, "end_turn").await;
        collect(started).await;
    }

    #[tokio::test]
    async fn reading_never_asks_and_plan_mode_refuses_a_change_without_asking() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let started = start(&conn, "look", true, false);
        let prompt = agent.prompt().await;
        let options = json!([{"kind": "allow_once", "optionId": "once"}, {"kind": "reject_once", "optionId": "no"}]);
        agent.send(json!({"jsonrpc": "2.0", "id": 60, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "r", "kind": "read"}, "options": options}})).await;
        assert_eq!(agent.recv().await["result"]["outcome"]["optionId"], "once");
        agent.send(json!({"jsonrpc": "2.0", "id": 61, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "e", "kind": "edit"}, "options": options}})).await;
        assert_eq!(agent.recv().await["result"]["outcome"]["optionId"], "no");
        finish_v1(&mut agent, &prompt, "end_turn").await;
        let (_, events) = collect(started).await;
        assert!(events.iter().all(|e| !matches!(e, AcpEvent::Permission(_))), "the user was never asked");
    }

    #[tokio::test]
    async fn bypass_allows_a_change_without_asking() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let started = start(&conn, "go", false, true);
        let prompt = agent.prompt().await;
        let options = json!([{"kind": "allow_once", "optionId": "once"}]);
        agent.send(json!({"jsonrpc": "2.0", "id": 62, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "e", "kind": "edit"}, "options": options}})).await;
        assert_eq!(agent.recv().await["result"]["outcome"]["optionId"], "once");
        finish_v1(&mut agent, &prompt, "end_turn").await;
        collect(started).await;
    }

    #[tokio::test]
    async fn stop_withdraws_a_waiting_permission_and_tells_the_agent_it_was_cancelled() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let mut started = start(&conn, "write", false, false);
        let prompt = agent.prompt().await;
        agent.send(json!({"jsonrpc": "2.0", "id": 70, "method": "session/request_permission", "params": {"toolCall": {"toolCallId": "t", "kind": "edit"}, "options": []}})).await;
        let Some(AcpEvent::Permission(_waiting)) = started.events.recv().await else { panic!("no request") };
        started.cancel.cancel();
        let mut seen = Vec::new();
        for _ in 0..3 {
            if let Some(frame) = agent.next_within(500).await {
                seen.push(frame);
            }
        }
        assert!(seen.iter().any(|f| f["method"] == "session/cancel"), "{seen:?}");
        assert!(seen.iter().any(|f| f["id"] == 70 && f["result"]["outcome"]["outcome"] == "cancelled"), "{seen:?}");
        finish_v1(&mut agent, &prompt, "cancelled").await;
        assert_eq!(collect(started).await.0.outcome, Outcome::Stopped);
    }

    #[tokio::test]
    async fn stopping_a_v2_session_cancels_then_closes_it() {
        let (conn, mut agent) = ready(2, json!({"session": {}})).await;
        *conn.session.lock().await = Some(SESSION.to_owned());
        let started = start(&conn, "go", false, false);
        let prompt = agent.prompt().await;
        agent.answer(&prompt["id"], json!({})).await;
        started.cancel.cancel();
        let mut methods = Vec::new();
        while methods.len() < 2 {
            let Some(frame) = agent.next_within(1000).await else { break };
            if let Some(method) = frame["method"].as_str() {
                methods.push(method.to_owned());
                if method == "session/close" {
                    agent.answer(&frame["id"], json!({})).await;
                }
            }
        }
        assert!(methods.contains(&"session/cancel".to_owned()) && methods.contains(&"session/close".to_owned()), "{methods:?}");
        agent.update(SESSION, json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": "cancelled"})).await;
        assert_eq!(collect(started).await.0.outcome, Outcome::Stopped);
        assert!(conn.session.lock().await.is_none(), "a closed session is not reused");
    }

    #[tokio::test]
    async fn a_turn_that_ignores_stop_is_ended_when_its_time_is_up() {
        let (conn, mut agent) = ready(1, json!({})).await;
        conn.stop_kill_after.store(50, Ordering::SeqCst);
        let started = start(&conn, "hang", false, false);
        let _prompt = agent.prompt().await;
        started.cancel.cancel();
        let (end, _) = collect(started).await;
        assert_eq!(end.outcome, Outcome::Stopped);
        assert!(!conn.is_alive(), "the agent is treated as gone, so the next turn starts a new one");
    }

    #[tokio::test]
    async fn a_peer_that_dies_after_stop_is_a_stopped_turn_not_an_error() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let started = start(&conn, "go", false, false);
        let _prompt = agent.prompt().await;
        started.cancel.cancel();
        drop(agent);
        assert_eq!(collect(started).await.0.outcome, Outcome::Stopped);
    }

    #[tokio::test]
    async fn a_failure_carries_what_the_agent_said_on_stderr() {
        let (conn, mut agent) = ready(1, json!({})).await;
        conn.remember_for_test("boom: cannot open the project");
        let started = start(&conn, "go", false, false);
        let _prompt = agent.prompt().await;
        drop(agent);
        let (end, _) = collect(started).await;
        let Outcome::Failed(why) = end.outcome else { panic!("not a failure") };
        assert!(why.contains("the agent exited") && why.contains("cannot open the project"), "{why}");
    }

    #[tokio::test]
    async fn the_conversation_before_the_agent_joined_is_handed_over_once() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let (tx, _events) = mpsc::unbounded_channel();
        let earlier = [Line { who: "User", text: "what is 2+2".into() }, Line { who: "Assistant", text: "4".into() }];
        let (turn_conn, who, stop) = (conn.clone(), harness("a"), CancellationToken::new());
        let turn = tokio::spawn(async move {
            let prompt = [PromptPart::Text("and 3+3?".into())];
            let request = TurnRequest { prompt: &prompt, earlier: &earlier, plan_mode: false, bypass: false, mcp_servers: &[], own: None };
            let thread = thread();
            let end = run(&turn_conn, &who, SESSION, &thread, &request, &tx, &stop).await;
            forget_session(&session_key(&thread, "a"), SESSION);
            end
        });
        let prompt = agent.prompt().await;
        let parts = prompt["params"]["prompt"].as_array().cloned().unwrap_or_default();
        assert!(parts[0]["text"].as_str().is_some_and(|t| t.contains("User: what is 2+2") && t.contains("Assistant: 4")));
        assert_eq!(parts[1]["text"], "and 3+3?");
        finish_v1(&mut agent, &prompt, "end_turn").await;
        let _ = turn.await;
        assert_eq!(conn.seen.load(Ordering::SeqCst), 3, "what it has seen now includes this exchange");
    }

    #[tokio::test]
    async fn a_slash_command_goes_first_with_nothing_before_it() {
        let (conn, mut agent) = ready(1, json!({})).await;
        let earlier = [Line { who: "User", text: "hi".into() }];
        let (tx, _events) = mpsc::unbounded_channel();
        let (turn_conn, who, stop) = (conn.clone(), harness("a"), CancellationToken::new());
        let turn = tokio::spawn(async move {
            let prompt = [PromptPart::Text("/review".into())];
            let request = TurnRequest { prompt: &prompt, earlier: &earlier, plan_mode: false, bypass: false, mcp_servers: &[], own: None };
            let thread = thread();
            let end = run(&turn_conn, &who, SESSION, &thread, &request, &tx, &stop).await;
            forget_session(&session_key(&thread, "a"), SESSION);
            end
        });
        let prompt = agent.prompt().await;
        assert_eq!(prompt["params"]["prompt"].as_array().map(Vec::len), Some(1));
        finish_v1(&mut agent, &prompt, "end_turn").await;
        let _ = turn.await;
    }

    #[test]
    fn a_v2_turn_is_settled_only_when_it_is_taken_quiet_and_idle() {
        assert!(turn_settled(Some("idle"), true, 0));
        assert!(!turn_settled(Some("idle"), false, 0), "not yet taken");
        assert!(!turn_settled(Some("idle"), true, 1), "a dialog is open");
        assert!(!turn_settled(Some("running"), true, 0));
        assert!(!turn_settled(None, true, 0));
        assert!(!turn_settled(Some("something new"), true, 0), "an unknown state is never idle");
    }

    #[test]
    fn a_turns_usage_counts_tokens_and_the_cost_it_added() {
        assert!(turn_usage(&json!({}), None).is_none());
        let usage = turn_usage(&json!({"usage": {"inputTokens": 10, "outputTokens": 5}}), Some(0.25));
        assert!(usage.as_ref().is_some_and(|u| u.input == 10 && u.output == 5 && u.total_tokens == 15));
        assert_eq!(usage.and_then(|u| u.cost).map(|c| c.total), Some(0.25));
    }
}
