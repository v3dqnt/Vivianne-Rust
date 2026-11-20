//! A running agent: a child process speaking JSON-RPC 2.0, one message per line over stdio.
//!
//! The connection is generic over its pipes so a test can run an agent inside the process (see `tests`).

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};

use super::info::SessionInfo;
use super::permission::cancelled_outcome;
use super::registry::{Harness, mark_ready, npx_cached_script};

/// The newest version of the agent protocol Vivianne speaks: v2.
const PROTOCOL_VERSION: u64 = 2;
/// The oldest it still speaks: v1 agents keep working, on v1's shapes.
const OLDEST_PROTOCOL_VERSION: u64 = 1;
/// `auth_required`: sign in, then ask again.
const AUTH_REQUIRED: i64 = -32000;
/// `unsupported_protocol_version`: the agent names the versions it does speak in `data`.
const UNSUPPORTED_VERSION: i64 = -32022;
/// An installed agent answers `initialize` at once; one still to be fetched through npx may take minutes.
const INITIALIZE_INSTALLED: Duration = Duration::from_secs(30);
const INITIALIZE_FETCHING: Duration = Duration::from_mins(4);
/// How long a turn gets after Stop to end itself before its process tree is ended.
pub(super) const STOP_KILL_AFTER: Duration = Duration::from_secs(5);
/// How much of the agent's stderr is kept to explain a failure.
const STDERR_TAIL_CHARS: usize = 4096;
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

/// A command that opens no console window (on Windows a console program started from a windowed app gets a
/// console of its own).
pub(super) fn quiet(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// A JSON-RPC error answer, kept as its parts: callers act on `code` (`-32000` asks for a sign-in, `-32022`
/// names the versions the agent does speak) and show `message`. A failure that never got an answer has code 0.
#[derive(Debug, Clone)]
pub(super) struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Value,
}

impl RpcError {
    /// A failure with no answer behind it: the pipe closed, a timeout, a bad frame.
    pub fn transport(message: impl Into<String>) -> Self {
        Self { code: 0, message: message.into(), data: Value::Null }
    }

    /// Whether the agent is saying it needs a sign-in before it will go on.
    pub fn is_auth_required(&self) -> bool {
        self.code == AUTH_REQUIRED
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.code == 0 { f.write_str(&self.message) } else { write!(f, "{} (code {})", self.message, self.code) }
    }
}

impl From<String> for RpcError {
    fn from(message: String) -> Self {
        Self::transport(message)
    }
}

// ── The connection ──────────────────────────────────────────────────────────

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;

/// What the agent sends while a prompt runs: its updates, and its requests for permission and for answers
/// to a form.
#[derive(Debug)]
pub(super) enum Incoming {
    Update(Value),
    Permission { id: Value, params: Value },
    Elicitation { id: Value },
}

pub(super) type Sink = Arc<Mutex<Option<mpsc::UnboundedSender<Incoming>>>>;

/// Ends the agent's whole process tree when the connection is dropped: `npx` and `.cmd` shims start the real
/// agent as a grandchild, which `kill_on_drop` alone would leave running.
pub(super) struct TreeKillOnDrop {
    pub pid: Option<u32>,
}

impl Drop for TreeKillOnDrop {
    fn drop(&mut self) {
        let Some(pid) = self.pid.take() else { return };
        kill_tree_now(pid);
    }
}

/// Ends a process and everything it started, without waiting.
fn kill_tree_now(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F", "/T"]).creation_flags(0x0800_0000).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-KILL", &format!("-{pid}")]).spawn();
    }
}

/// Ends a process and everything it started, and waits for that to be done.
pub(super) async fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = quiet("taskkill").args(["/PID", &pid.to_string(), "/F", "/T"]).output().await;
    }
    #[cfg(not(windows))]
    {
        let _ = quiet("kill").args(["-KILL", &format!("-{pid}")]).output().await;
    }
}

pub(super) struct Conn {
    stdin: tokio::sync::Mutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: Pending,
    next: AtomicU64,
    /// Where updates go while a prompt runs.
    pub sink: Sink,
    pub alive: Arc<AtomicBool>,
    pub session: tokio::sync::Mutex<Option<String>>,
    /// The protocol version agreed in `initialize`: 0 until it answers.
    version: AtomicU64,
    /// What it said it can do, in its answer to `initialize`.
    caps: OnceLock<Value>,
    /// How many of the thread's messages its session has been told about.
    pub seen: AtomicUsize,
    /// The session's running cost as it last reported it, to take a turn's share from.
    pub cost: Mutex<f64>,
    /// What the agent has told us about the session: its commands, mode, options and title.
    pub info: Mutex<SessionInfo>,
    /// The process, when there is one. A test agent has none.
    _child: tokio::sync::Mutex<Option<tokio::process::Child>>,
    tree: TreeKillOnDrop,
    /// The end of what it wrote on stderr, kept to explain a failure.
    stderr: Mutex<String>,
    /// Flips to `true` when its stderr reaches EOF, so a failure can wait for the last lines.
    pub stderr_done: watch::Sender<bool>,
    /// How long after Stop the turn gets before its process tree is ended; a field so a test can shorten it.
    pub stop_kill_after: AtomicU64,
}

impl Conn {
    async fn write(&self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await.map_err(|e| format!("the agent stopped listening: {e}"))?;
        stdin.flush().await.map_err(|e| e.to_string())
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, RpcError> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        trace("harness", format!("-> {method} #{id}"));
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(PoisonError::into_inner).insert(id, tx);
        let sent = self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })).await;
        if let Err(why) = sent {
            self.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
            return Err(why.into());
        }
        let answer = match timeout {
            Some(limit) => {
                let Ok(answer) = tokio::time::timeout(limit, rx).await else {
                    // Nothing will wait for this answer now, so its slot must not stay.
                    self.pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
                    return Err(RpcError::transport(format!("the agent didn't answer {method} in {}s", limit.as_secs())));
                };
                answer
            }
            None => rx.await,
        };
        answer.map_err(|_| RpcError::transport("the agent exited"))?
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

    /// The version the agent agreed to in `initialize`; 0 before it answers.
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    pub fn speaks_v2(&self) -> bool {
        self.version() >= 2
    }

    /// The agent's answer to `initialize`.
    pub fn caps(&self) -> Value {
        self.caps.get().cloned().unwrap_or(Value::Null)
    }

    /// Whether the agent advertises the capability at `path`. Every capability is an object read by presence
    /// (`{}` is support) or, where a version spells one as a boolean, `true`.
    pub fn flag(&self, path: &str) -> bool {
        self.caps.get().and_then(|c| c.pointer(path)).is_some_and(|v| v.is_object() || v.as_bool() == Some(true))
    }

    /// A capability whose home moved between versions.
    pub fn cap(&self, v2: &str, v1: &str) -> bool {
        self.flag(if self.speaks_v2() { v2 } else { v1 })
    }

    pub fn takes_images(&self) -> bool {
        self.cap("/capabilities/session/prompt/image", "/agentCapabilities/promptCapabilities/image")
    }

    /// The agent's process id, when it runs as one.
    pub fn pid(&self) -> Option<u32> {
        self.tree.pid
    }

    /// Keeps the newest of what it wrote on stderr, up to `STDERR_TAIL_CHARS` characters: the last lines are
    /// what explain a failure.
    fn remember_stderr(&self, line: &str) {
        let mut tail = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
        tail.push_str(line);
        tail.push('\n');
        let chars = tail.chars().count();
        if chars > STDERR_TAIL_CHARS {
            *tail = tail.chars().skip(chars - STDERR_TAIL_CHARS).collect();
        }
    }

    #[cfg(test)]
    pub fn remember_for_test(&self, line: &str) {
        self.remember_stderr(line);
    }

    pub fn stderr_tail(&self) -> String {
        self.stderr.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Whether the agent's process is still there.
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

// ── Starting one ────────────────────────────────────────────────────────────

/// The environment the agent starts with: its registry entry's, plus the switch the Claude adapter reads to
/// allow ACP v2. A value the user set wins.
fn launch_env(harness: &Harness) -> HashMap<String, String> {
    let mut env = harness.env.clone();
    if harness.id == "claude-acp" {
        env.entry("CLAUDE_AGENT_ACP_EXPERIMENTAL_V2".to_owned()).or_insert_with(|| "1".to_owned());
    }
    env
}

/// What to run for this agent: an npx agent already in npx's cache starts straight from it with `node`;
/// anything else through its launcher, found on the PATH (`npx.cmd` on Windows).
fn command_for(harness: &Harness) -> Result<(PathBuf, Vec<String>), String> {
    let cached = if harness.launcher == "npx" { harness.package().and_then(npx_cached_script) } else { None };
    if let Some(script) = cached {
        let node = which::which("node").map_err(|_| format!("{} needs Node.js, which isn't on this computer's PATH.", harness.name))?;
        // Whatever follows `-y <package>` is the agent's own arguments.
        let package = harness.package().unwrap_or_default();
        let rest = harness.args.iter().skip_while(|a| a.as_str() != package).skip(1).cloned();
        return Ok((node, std::iter::once(script.to_string_lossy().into_owned()).chain(rest).collect()));
    }
    let program = which::which(&harness.launcher).map_err(|_| format!("{} runs through {}, which isn't installed.", harness.name, harness.launcher))?;
    Ok((program, harness.args.clone()))
}

/// Starts the agent, reads what it writes, and runs `initialize`.
pub(super) async fn spawn(harness: &Harness, cwd: &str) -> Result<Arc<Conn>, String> {
    let (program, args) = command_for(harness)?;
    let mut command = quiet(&program);
    command
        .args(&args)
        .envs(launch_env(harness))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    trace(&harness.name, format!("starting: {} {} (in {cwd})", program.display(), args.join(" ")));
    let mut child = command.spawn().map_err(|e| format!("Couldn't start {} ({}): {e}", harness.name, program.display()))?;
    let pid = child.id();
    let stdin = child.stdin.take().ok_or("the agent has no input")?;
    let stdout = child.stdout.take().ok_or("the agent has no output")?;
    let stderr = child.stderr.take().ok_or("the agent has no error output")?;

    let conn = wire(Box::new(stdin), Box::new(BufReader::new(stdout)), Some(child), TreeKillOnDrop { pid }, harness.name.clone());

    // Its stderr is its log; the last lines also explain a failure, so they are kept on the connection. The
    // reader holds it weakly: it must not keep a connection alive past the turn that ends it.
    let (name, weak) = (harness.name.clone(), Arc::downgrade(&conn));
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            trace(&name, format!("stderr: {line}"));
            if let Some(conn) = weak.upgrade() {
                conn.remember_stderr(&line);
            }
        }
        if let Some(conn) = weak.upgrade() {
            let _ = conn.stderr_done.send_replace(true);
        }
    });

    handshake(&conn, harness).await?;
    mark_ready(&harness.id);
    Ok(conn)
}

/// A connection over a pair of pipes: an agent's stdio when it runs as a child process, an in-process peer in
/// tests. The reader starts at once, so answers and notifications arrive while the caller is still sending.
pub(super) fn wire(
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    child: Option<tokio::process::Child>,
    tree: TreeKillOnDrop,
    who: String,
) -> Arc<Conn> {
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
        version: AtomicU64::new(0),
        caps: OnceLock::new(),
        seen: AtomicUsize::new(0),
        cost: Mutex::new(0.0),
        info: Mutex::new(SessionInfo::default()),
        _child: tokio::sync::Mutex::new(child),
        tree,
        stderr: Mutex::new(String::new()),
        stderr_done: watch::channel(false).0,
        stop_kill_after: AtomicU64::new(u64::try_from(STOP_KILL_AFTER.as_millis()).unwrap_or(u64::MAX)),
    });
    tokio::spawn(read_loop(stdout, Arc::downgrade(&conn), pending, sink, alive, who));
    conn
}

/// Everything the agent writes on stdout: answers to our requests, its own requests, and `session/update`
/// notifications.
async fn read_loop(reader: Box<dyn AsyncBufRead + Unpin + Send>, conn: Weak<Conn>, pending: Pending, sink: Sink, alive: Arc<AtomicBool>, who: String) {
    let mut lines = reader.lines();
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
        let _ = waiting.send(Err(RpcError::transport("the agent exited")));
    }
}

fn answer(pending: &Pending, id: &Value, msg: &Value) {
    let waiting = id.as_u64().and_then(|n| pending.lock().unwrap_or_else(PoisonError::into_inner).remove(&n));
    let Some(waiting) = waiting else { return };
    let outcome = match msg.get("error") {
        Some(e) => Err(RpcError {
            code: e.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: e.get("message").and_then(Value::as_str).unwrap_or("error").to_owned(),
            data: e.get("data").cloned().unwrap_or(Value::Null),
        }),
        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
    };
    let _ = waiting.send(outcome);
}

/// A request from the agent. A permission or a form goes to whoever is running the prompt; nothing else is
/// offered (no file access, no terminal), and a permission or a form asked outside a prompt is answered as
/// cancelled.
async fn ask(conn: &Weak<Conn>, sink: &Sink, method: &str, id: Value, params: Value) {
    if matches!(method, "session/request_permission" | "elicitation/create") {
        let item = if method == "elicitation/create" { Incoming::Elicitation { id: id.clone() } } else { Incoming::Permission { id: id.clone(), params } };
        let sent = sink.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(|s| s.send(item).is_ok());
        if sent == Some(true) {
            return;
        }
    }
    let Some(conn) = conn.upgrade() else { return };
    let _ = match method {
        "session/request_permission" => conn.reply(id, cancelled_outcome()).await,
        "elicitation/create" => conn.reply(id, json!({ "action": "cancel" })).await,
        other => conn.refuse(id, other).await,
    };
}

// ── The first exchange ──────────────────────────────────────────────────────

/// What `initialize` sends: the newest version Vivianne speaks, and who we are. Vivianne offers no client
/// capabilities: the agent does its own file and shell work.
fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "info": { "name": "vivianne", "title": "Vivianne", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// What an agent's refusal to `initialize` means. A version mismatch names the versions it does speak, which
/// is more use than its code.
fn explain_initialize_failure(harness: &Harness, error: &RpcError) -> String {
    let supported: Vec<String> = error
        .data
        .get("supported")
        .and_then(Value::as_array)
        .map(|v| v.iter().filter_map(Value::as_u64).map(|n| n.to_string()).collect())
        .unwrap_or_default();
    if error.code == UNSUPPORTED_VERSION && !supported.is_empty() {
        return format!(
            "{} only speaks version {} of the agent protocol, which this version of Vivianne doesn't. Update Vivianne or the agent.",
            harness.name,
            supported.join(", ")
        );
    }
    format!("{} didn't start: {error}", harness.name)
}

/// Agrees the protocol version and learns what the agent can do. The answer is kept on the connection as
/// `caps`, the version as `version`.
pub(super) async fn handshake(conn: &Conn, harness: &Harness) -> Result<Value, String> {
    let limit = if harness.managed { INITIALIZE_INSTALLED } else { INITIALIZE_FETCHING };
    let hello = conn.request("initialize", initialize_params(), Some(limit)).await.map_err(|e| explain_initialize_failure(harness, &e))?;
    let version = hello
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{} answered `initialize` without a protocol version. Update it, or update Vivianne.", harness.name))?;
    if !(OLDEST_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version) {
        return Err(format!(
            "{} speaks version {version} of the agent protocol; Vivianne speaks {OLDEST_PROTOCOL_VERSION} to {PROTOCOL_VERSION}. Update Vivianne or the agent.",
            harness.name
        ));
    }
    conn.version.store(version, Ordering::SeqCst);
    let _ = conn.caps.set(hello.clone());
    trace(&harness.name, format!("initialized: protocol version {version}"));
    Ok(hello)
}

#[cfg(test)]
pub(super) mod testing {
    //! An agent that runs inside the test process, writing and reading the same frames a real one does.

    use serde_json::Value;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    use super::*;

    /// The agent's end of the two pipes.
    pub struct Peer {
        out: DuplexStream,
        inp: BufReader<DuplexStream>,
    }

    /// A connection to an in-process agent.
    pub fn peer(name: &str) -> (Arc<Conn>, Peer) {
        let (to_agent, from_vivianne) = tokio::io::duplex(1 << 16);
        let (to_vivianne, from_agent) = tokio::io::duplex(1 << 16);
        let conn = wire(Box::new(to_agent), Box::new(BufReader::new(from_agent)), None, TreeKillOnDrop { pid: None }, name.to_owned());
        (conn, Peer { out: to_vivianne, inp: BufReader::new(from_vivianne) })
    }

    impl Peer {
        /// The next message Vivianne sends. Panics rather than hang the suite.
        pub async fn recv(&mut self) -> Value {
            let mut line = String::new();
            let read = tokio::time::timeout(Duration::from_secs(10), self.inp.read_line(&mut line)).await;
            assert!(matches!(read, Ok(Ok(n)) if n > 0), "Vivianne sent nothing");
            serde_json::from_str(&line).unwrap_or(Value::Null)
        }

        /// The next frame if Vivianne sends one within `ms`; `None` when it stays quiet.
        pub async fn next_within(&mut self, ms: u64) -> Option<Value> {
            let mut line = String::new();
            match tokio::time::timeout(Duration::from_millis(ms), self.inp.read_line(&mut line)).await {
                Ok(Ok(n)) if n > 0 => serde_json::from_str(&line).ok(),
                _ => None,
            }
        }

        pub async fn send(&mut self, msg: Value) {
            let mut line = serde_json::to_string(&msg).unwrap_or_default();
            line.push('\n');
            let _ = self.out.write_all(line.as_bytes()).await;
            let _ = self.out.flush().await;
        }

        pub async fn answer(&mut self, id: &Value, result: Value) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "result": result })).await;
        }

        pub async fn refuse(&mut self, id: &Value, code: i64, message: &str, data: Value) {
            self.send(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message, "data": data } })).await;
        }

        /// Answers `initialize` as an agent of `version`, in that version's own shape, and returns what
        /// Vivianne asked with.
        pub async fn hello_version(&mut self, version: u64, capabilities: Value, auth: Value) -> Value {
            let msg = self.recv().await;
            assert_eq!(msg["method"], "initialize", "expected initialize, got {msg}");
            let answer = if version >= 2 {
                json!({ "protocolVersion": version, "capabilities": capabilities, "info": { "name": "mock", "version": "0.0.0" }, "authMethods": auth })
            } else {
                json!({ "protocolVersion": version, "agentCapabilities": capabilities, "authMethods": auth })
            };
            self.answer(&msg["id"], answer).await;
            msg
        }

        /// Answers `initialize` as a v2 agent that does everything Vivianne asks for.
        pub async fn hello(&mut self) -> Value {
            self.hello_version(2, json!({ "session": { "prompt": { "image": {} } } }), json!([])).await
        }

        /// Answers `session/new` and returns what Vivianne asked with.
        pub async fn open(&mut self, session: &str) -> Value {
            let msg = self.recv().await;
            assert_eq!(msg["method"], "session/new", "expected session/new, got {msg}");
            self.answer(&msg["id"], json!({ "sessionId": session })).await;
            msg
        }

        /// The turn's `session/prompt`, left unanswered so the test can stream updates first.
        pub async fn prompt(&mut self) -> Value {
            let msg = self.recv().await;
            assert_eq!(msg["method"], "session/prompt", "expected session/prompt, got {msg}");
            msg
        }

        pub async fn update(&mut self, session: &str, update: Value) {
            self.send(json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": update } })).await;
        }
    }

    pub fn harness(id: &str) -> Harness {
        Harness {
            id: id.to_owned(),
            name: "Mock".to_owned(),
            description: String::new(),
            icon: None,
            available: true,
            ready: true,
            launcher: "mock".to_owned(),
            args: Vec::new(),
            env: HashMap::new(),
            package: None,
            extra: Vec::new(),
            managed: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{harness, peer};
    use super::*;

    #[tokio::test]
    async fn initialize_agrees_the_version_and_keeps_what_the_agent_can_do() {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { handshake(&conn, &harness("a")).await }
        });
        let hello = agent.hello().await;
        assert_eq!(hello["params"]["protocolVersion"], 2);
        assert_eq!(hello["params"]["info"]["name"], "vivianne");
        assert!(asked.await.is_ok_and(|r| r.is_ok()));
        assert!(conn.speaks_v2() && conn.takes_images());
    }

    #[tokio::test]
    async fn a_v1_agent_is_read_from_v1s_fields() {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { handshake(&conn, &harness("a")).await }
        });
        agent.hello_version(1, json!({ "promptCapabilities": { "image": true } }), json!([])).await;
        assert!(asked.await.is_ok_and(|r| r.is_ok()));
        assert_eq!(conn.version(), 1);
        assert!(conn.takes_images());
    }

    #[tokio::test]
    async fn an_agent_that_only_speaks_a_newer_version_names_it() {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { handshake(&conn, &harness("a")).await }
        });
        let msg = agent.recv().await;
        agent.refuse(&msg["id"], UNSUPPORTED_VERSION, "unsupported", json!({ "supported": [3, 4] })).await;
        let failure = asked.await.unwrap_or(Err(String::new())).err().unwrap_or_default();
        assert!(failure.contains("version 3, 4"), "{failure}");
    }

    #[tokio::test]
    async fn an_answer_with_a_version_vivianne_does_not_speak_is_refused() {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { handshake(&conn, &harness("a")).await }
        });
        agent.hello_version(9, json!({}), json!([])).await;
        let failure = asked.await.unwrap_or(Err(String::new())).err().unwrap_or_default();
        assert!(failure.contains("version 9"), "{failure}");
    }

    #[tokio::test]
    async fn a_request_that_times_out_leaves_no_waiting_slot_behind() {
        let (conn, mut agent) = peer("mock");
        let result = conn.request("slow", json!({}), Some(Duration::from_millis(30))).await;
        assert!(result.is_err_and(|e| e.message.contains("didn't answer slow")));
        assert!(conn.pending.lock().unwrap_or_else(PoisonError::into_inner).is_empty());
        let _ = agent.recv().await;
    }

    #[tokio::test]
    async fn a_permission_asked_outside_a_prompt_is_answered_as_cancelled() {
        let (_conn, mut agent) = peer("mock");
        agent.send(json!({ "jsonrpc": "2.0", "id": 7, "method": "session/request_permission", "params": {} })).await;
        let answer = agent.recv().await;
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["result"]["outcome"]["outcome"], "cancelled");
    }

    #[tokio::test]
    async fn requests_nothing_offers_are_refused_and_the_agents_exit_ends_waiting_calls() {
        let (conn, mut agent) = peer("mock");
        agent.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "fs/read_text_file", "params": {} })).await;
        let refused = agent.recv().await;
        assert_eq!(refused["error"]["code"], -32601);
        let waiting = tokio::spawn({
            let conn = conn.clone();
            async move { conn.request("later", json!({}), None).await }
        });
        let _ = agent.recv().await;
        drop(agent);
        let ended = waiting.await.map_or(None, |r| r.map(|_| ()).err().map(|e| e.message));
        assert_eq!(ended.as_deref(), Some("the agent exited"));
        assert!(!conn.is_alive());
    }

    #[test]
    fn the_stderr_tail_keeps_only_the_newest() {
        let (stdin, _keep) = tokio::io::duplex(8);
        let (_unused, stdout) = tokio::io::duplex(8);
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        let Ok(runtime) = runtime else { return };
        runtime.block_on(async {
            let conn = wire(Box::new(stdin), Box::new(BufReader::new(stdout)), None, TreeKillOnDrop { pid: None }, "t".into());
            for n in 0..2000 {
                conn.remember_stderr(&format!("line {n}"));
            }
            let tail = conn.stderr_tail();
            assert!(tail.chars().count() <= STDERR_TAIL_CHARS);
            assert!(tail.contains("line 1999") && !tail.contains("line 0\n"));
        });
    }
}
