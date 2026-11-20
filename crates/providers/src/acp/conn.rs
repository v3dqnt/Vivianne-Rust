//! A running agent: a child process speaking JSON-RPC 2.0, one message per line over stdio.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use super::registry::{Harness, mark_ready};

const PROTOCOL_VERSION: u64 = 1;
/// The first start downloads the agent through npx.
const INITIALIZE_TIMEOUT: Duration = Duration::from_mins(4);
const TRACE_LINES: usize = 400;
const TRACE_LINE_CHARS: usize = 400;

// ── What happened, for finding out why one is stuck ─────────────────────────

/// The last lines of traffic with any agent: what we sent, what it wrote on stdout and stderr, and the stages
/// of a turn.
static TRACE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

pub(super) fn trace(who: &str, line: impl AsRef<str>) {
    let short: String = line.as_ref().chars().take(TRACE_LINE_CHARS).collect();
    let mut lines = TRACE.lock().unwrap_or_else(PoisonError::into_inner);
    if lines.len() >= TRACE_LINES {
        lines.pop_front();
    }
    lines.push_back(format!("{} [{who}] {short}", clock()));
}

/// The trace so far, oldest first.
#[must_use]
pub fn trace_lines() -> Vec<String> {
    TRACE.lock().unwrap_or_else(PoisonError::into_inner).iter().cloned().collect()
}

/// The time of day (UTC) as `hh:mm:ss`.
fn clock() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86_400;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

// ── The connection ──────────────────────────────────────────────────────────

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// What the agent sends while a prompt runs: its updates, and its requests for permission.
#[derive(Debug)]
pub(super) enum Incoming {
    Update(Value),
    Permission { id: Value, params: Value },
}

pub(super) type Sink = Arc<Mutex<Option<mpsc::UnboundedSender<Incoming>>>>;

pub(super) struct Conn {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    pending: Pending,
    next: AtomicU64,
    /// Where updates go while a prompt runs.
    pub sink: Sink,
    pub alive: Arc<AtomicBool>,
    pub session: tokio::sync::Mutex<Option<String>>,
    /// Held so the process ends when the connection is dropped.
    _child: tokio::sync::Mutex<tokio::process::Child>,
}

impl Conn {
    async fn write(&self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| format!("the agent stopped listening: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        trace("harness", format!("-> {method} #{id}"));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).insert(id, tx);
        let sent = self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
        if let Err(why) = sent {
            self.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
            return Err(why);
        }
        let answer = match timeout {
            Some(limit) => {
                let Ok(answer) = tokio::time::timeout(limit, rx).await else {
                    // Nothing will wait for this answer now, so its slot must not stay.
                    self.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
                    return Err(format!("the agent didn't answer {method} in {}s", limit.as_secs()));
                };
                answer
            }
            None => rx.await,
        };
        answer.map_err(|_| "the agent exited".to_owned())?
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params })).await
    }

    pub async fn reply(&self, id: Value, result: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result })).await
    }

    async fn refuse(&self, id: Value, method: &str) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("{method} isn't supported") } })).await
    }
}

/// Starts the agent, reads what it writes, and runs `initialize`.
pub(super) async fn spawn(harness: &Harness, cwd: &str) -> Result<Arc<Conn>, String> {
    let program = harness.program().ok_or_else(|| format!("{} runs through {}, which isn't installed.", harness.name, harness.launcher))?;
    let mut command = tokio::process::Command::new(&program);
    command
        .args(&harness.args)
        .envs(&harness.env)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    trace(&harness.name, format!("starting: {} {} (in {cwd})", program.display(), harness.args.join(" ")));
    let mut child = command.spawn().map_err(|e| format!("Couldn't start {} ({}): {e}", harness.name, program.display()))?;
    let stdin = child.stdin.take().ok_or("the agent has no input")?;
    let stdout = child.stdout.take().ok_or("the agent has no output")?;
    let stderr = child.stderr.take().ok_or("the agent has no error output")?;

    let pending: Pending = Arc::default();
    let sink: Sink = Arc::default();
    let alive = Arc::new(AtomicBool::new(true));
    let conn = Arc::new(Conn {
        stdin: tokio::sync::Mutex::new(stdin),
        pending: pending.clone(),
        next: AtomicU64::new(1),
        sink: sink.clone(),
        alive: alive.clone(),
        session: tokio::sync::Mutex::new(None),
        _child: tokio::sync::Mutex::new(child),
    });

    // Its stderr is its log; the last lines explain a failed start.
    let name = harness.name.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            trace(&name, format!("stderr: {line}"));
        }
    });
    tokio::spawn(read_output(stdout, Arc::downgrade(&conn), pending, sink, alive, harness.name.clone()));

    conn.request(
        "initialize",
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "Vivianne", "version": env!("CARGO_PKG_VERSION") },
        }),
        Some(INITIALIZE_TIMEOUT),
    )
    .await
    .map_err(|e| format!("{} didn't start: {e}", harness.name))?;
    mark_ready(&harness.id);
    Ok(conn)
}

/// Everything the agent writes on stdout: answers, requests, notifications.
async fn read_output(
    stdout: tokio::process::ChildStdout,
    conn: std::sync::Weak<Conn>,
    pending: Pending,
    sink: Sink,
    alive: Arc<AtomicBool>,
    who: String,
) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            trace(&who, format!("stdout (not JSON): {line}"));
            continue;
        };
        // Streamed text would drown everything else; the rest is shown.
        let is_chunk = msg.pointer("/params/update/sessionUpdate").and_then(Value::as_str).is_some_and(|k| k.ends_with("_chunk"));
        if !is_chunk {
            trace(&who, format!("<- {line}"));
        }
        let method = msg.get("method").and_then(Value::as_str);
        let id = msg.get("id").cloned();
        match (method, id) {
            // An answer to one of ours.
            (None, Some(id)) => answer(&pending, &id, &msg),
            // It asks us something.
            (Some(method), Some(id)) => ask(&conn, &sink, method, id, msg.get("params").cloned().unwrap_or(Value::Null)).await,
            // A notification: the session's progress.
            (Some("session/update"), None) => {
                if let Some(sink) = sink.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    let _ = sink.send(Incoming::Update(msg.get("params").cloned().unwrap_or(Value::Null)));
                }
            }
            _ => {}
        }
    }
    alive.store(false, Ordering::SeqCst);
    trace(&who, "the agent's output closed: it exited");
    // Whatever was waiting will never be answered now.
    for (_, waiting) in pending.lock().unwrap_or_else(PoisonError::into_inner).drain() {
        let _ = waiting.send(Err("the agent exited".to_owned()));
    }
}

fn answer(pending: &Pending, id: &Value, msg: &Value) {
    let waiting = id.as_u64().and_then(|n| pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&n));
    let Some(waiting) = waiting else { return };
    let outcome = match msg.get("error") {
        Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_owned()),
        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = waiting.send(outcome);
}

/// A request from the agent. A permission goes to whoever is running the prompt; nothing else is offered (no
/// file access, no terminal), and a permission asked outside a prompt is refused.
async fn ask(conn: &std::sync::Weak<Conn>, sink: &Sink, method: &str, id: Value, params: Value) {
    if method == "session/request_permission" {
        let sent = sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|s| s.send(Incoming::Permission { id: id.clone(), params }).is_ok());
        if sent == Some(true) {
            return;
        }
    }
    let Some(conn) = conn.upgrade() else { return };
    let _ = if method == "session/request_permission" {
        conn.reply(id, json!({ "outcome": { "outcome": "cancelled" } })).await
    } else {
        conn.refuse(id, method).await
    };
}
