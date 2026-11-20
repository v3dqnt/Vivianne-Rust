//! Other coding agents (Claude Code, Codex, Gemini CLI and the rest of the Agent Client Protocol registry) as
//! something a thread can talk to.
//!
//! The agent runs as a child process speaking ACP (JSON-RPC 2.0, one message per line over stdio):
//! `initialize`, then `session/new` once per thread, then `session/prompt` per turn while it streams
//! `session/update` notifications back. [`Acp::run_turn`] turns those into [`AcpEvent`]s.
//!
//! - `registry.rs`: which agents exist and how to start them.
//! - `conn.rs`: a running agent and the trace of its traffic.
//! - `turn.rs`: one turn.

mod conn;
mod registry;
mod turn;

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use conn::{Conn, Incoming, spawn, trace};
pub use conn::trace_lines;
pub use registry::{Harness, find, harnesses};
pub use turn::{AcpEvent, Outcome, PermissionRequest, PromptPart, ToolInfo};

/// The `provider` a model served by an agent carries.
pub const PROVIDER: &str = "harness";

/// How long setup may take (the package is downloaded).
const INSTALL_TIMEOUT: Duration = Duration::from_mins(5);
/// How long a test prompt may take.
const DEBUG_PROMPT_TIMEOUT: Duration = Duration::from_secs(90);
const DEBUG_SESSION_TIMEOUT: Duration = Duration::from_mins(1);

/// The running agents, one per thread and agent, kept between turns so the conversation carries on in the same
/// session.
#[derive(Default)]
pub struct Acp {
    conns: Mutex<HashMap<(String, String), Arc<Conn>>>,
}

impl std::fmt::Debug for Acp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Acp").finish_non_exhaustive()
    }
}

impl Acp {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Runs one turn of `thread` on agent `harness_id` in `cwd`, streaming what it does into `events`.
    /// Stopping through `cancel` asks the agent to stop.
    pub async fn run_turn(
        &self,
        thread: &str,
        harness_id: &str,
        cwd: &str,
        prompt: &[PromptPart],
        events: &UnboundedSender<AcpEvent>,
        cancel: &CancellationToken,
    ) -> Result<Outcome, String> {
        let harness = find(harness_id).await?;
        if !harness.available {
            return Err(format!("{} runs through {}, which isn't installed. Install Node.js (for npx) or uv (for uvx) and try again.", harness.name, harness.launcher));
        }
        trace(&harness.name, format!("turn: connecting (thread {thread})"));
        let conn = self.connection(thread, &harness, cwd).await?;
        let session = turn::session_of(&conn, &harness, cwd).await?;
        let outcome = turn::run(&conn, &harness, &session, prompt, events, cancel).await;
        if !conn.alive.load(Ordering::SeqCst) {
            // The next turn starts a fresh agent.
            self.conns.lock().unwrap_or_else(PoisonError::into_inner).remove(&(thread.to_owned(), harness.id.clone()));
        }
        outcome
    }

    async fn connection(&self, thread: &str, harness: &Harness, cwd: &str) -> Result<Arc<Conn>, String> {
        let key = (thread.to_owned(), harness.id.clone());
        let existing = self.conns.lock().unwrap_or_else(PoisonError::into_inner).get(&key).filter(|c| c.alive.load(Ordering::SeqCst)).cloned();
        if let Some(conn) = existing {
            return Ok(conn);
        }
        let conn = spawn(harness, cwd).await?;
        self.conns.lock().unwrap_or_else(PoisonError::into_inner).insert(key, conn.clone());
        Ok(conn)
    }

    /// Drops every running agent, so the next turn starts a fresh one: the way out of a stuck one. Returns how
    /// many there were.
    pub fn reset(&self) -> usize {
        let mut conns = self.conns.lock().unwrap_or_else(PoisonError::into_inner);
        let count = conns.len();
        conns.clear(); // the processes end when their connections are dropped
        drop(conns);
        trace("harness", format!("reset: dropped {count} running agent(s)"));
        count
    }
}

/// Sets an agent up: runs its launcher once with no input, which fetches the package into the launcher's cache;
/// the agent exits when it finds nothing to read. After that, a turn starts at once.
pub async fn install(id: &str) -> Result<String, String> {
    let harness = find(id).await?;
    let program = harness
        .program()
        .ok_or_else(|| format!("{} runs through {}, which isn't installed. Install Node.js (for npx) or uv (for uvx) first.", harness.name, harness.launcher))?;
    let mut command = tokio::process::Command::new(&program);
    command.args(&harness.args).envs(&harness.env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    trace(&harness.name, format!("setting up: {} {}", program.display(), harness.args.join(" ")));
    let child = command.spawn().map_err(|e| format!("Couldn't run {}: {e}", program.display()))?;
    let output = tokio::time::timeout(INSTALL_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{} took over 5 minutes to download.", harness.name))?
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail = last_lines(&stderr, 6);
    trace(&harness.name, format!("setup finished ({}): {tail}", output.status));
    // The agent may exit non-zero on empty input; a failure to *fetch* is what matters.
    if !output.status.success() && fetch_failed(&stderr) {
        return Err(format!("Couldn't download {}:\n{tail}", harness.name));
    }
    registry::mark_ready(&harness.id);
    Ok(format!("{} is set up.", harness.name))
}

fn fetch_failed(stderr: &str) -> bool {
    ["E404", "ENOTFOUND", "ETIMEDOUT", "code E", "not found in the registry"].iter().any(|sign| stderr.contains(sign))
}

fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.contains("npm warn")).collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

// ── Testing an agent by hand ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    pub ok: bool,
    pub millis: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct DebugReport {
    pub steps: Vec<Step>,
    /// What the agent said to the test prompt.
    pub reply: String,
    pub trace: Vec<String>,
}

fn step(name: &str, ok: bool, since: Instant, detail: String) -> Step {
    Step { name: name.to_owned(), ok, millis: u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX), detail }
}

/// Starts the agent fresh and walks it through what a turn needs, timing each step and saying what failed:
/// start and `initialize`, `session/new`, then one short prompt. Nothing here touches a thread.
pub async fn debug(id: &str, prompt: Option<String>) -> Result<DebugReport, String> {
    let harness = find(id).await?;
    let mut report = DebugReport::default();
    let started = Instant::now();
    let launcher = harness.available;
    report.steps.push(step(
        "Launcher found",
        launcher,
        started,
        if launcher { format!("{} {}", harness.launcher, harness.args.join(" ")) } else { format!("{} isn't installed", harness.launcher) },
    ));
    if launcher {
        walk(&harness, prompt, &mut report).await;
    }
    report.trace = trace_lines();
    Ok(report)
}

async fn walk(harness: &Harness, prompt: Option<String>, report: &mut DebugReport) {
    let cwd = std::env::current_dir().map_or_else(|_| ".".to_owned(), |d| d.display().to_string());
    trace(&harness.name, "debug: starting");

    let t = Instant::now();
    let conn = match spawn(harness, &cwd).await {
        Ok(conn) => {
            report.steps.push(step("Start and initialize", true, t, "answered `initialize`".to_owned()));
            conn
        }
        Err(why) => {
            report.steps.push(step("Start and initialize", false, t, why));
            return;
        }
    };

    let t = Instant::now();
    let opened = conn.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(DEBUG_SESSION_TIMEOUT)).await;
    let session = match opened.as_ref().map(|r| r.get("sessionId").and_then(serde_json::Value::as_str)) {
        Ok(Some(session)) => {
            report.steps.push(step("Open a session", true, t, format!("session {session}")));
            session.to_owned()
        }
        Ok(None) => {
            report.steps.push(step("Open a session", false, t, "it answered without a session id".to_owned()));
            return;
        }
        Err(why) => {
            report.steps.push(step("Open a session", false, t, why.clone()));
            return;
        }
    };

    let t = Instant::now();
    let text = prompt.unwrap_or_else(|| "Reply with the single word: ok".to_owned());
    let outcome = ask_once(&conn, &harness.name, &session, &text, &mut report.reply).await;
    match outcome {
        Ok(stop) => report.steps.push(step("Answer a prompt", true, t, format!("stop reason: {stop}"))),
        Err(why) => report.steps.push(step("Answer a prompt", false, t, why)),
    }
}

/// One prompt, collecting the reply. A permission asked here is a failure to report, not a thing to grant.
async fn ask_once(conn: &Arc<Conn>, name: &str, session: &str, text: &str, reply: &mut String) -> Result<String, String> {
    let (tx, mut incoming) = mpsc::unbounded_channel();
    *conn.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
    let (prompt_conn, session_id, text) = (conn.clone(), session.to_owned(), text.to_owned());
    let mut done = tokio::spawn(async move {
        prompt_conn.request("session/prompt", json!({ "sessionId": session_id, "prompt": [{ "type": "text", "text": text }] }), None).await
    });
    let deadline = tokio::time::sleep(DEBUG_PROMPT_TIMEOUT);
    tokio::pin!(deadline);
    let outcome = loop {
        tokio::select! {
            result = &mut done => break result.unwrap_or_else(|e| Err(e.to_string())),
            () = &mut deadline => {
                let _ = conn.notify("session/cancel", json!({ "sessionId": session })).await;
                done.abort();
                break Err("no answer in 90s".to_owned());
            }
            Some(message) = incoming.recv() => match message {
                Incoming::Update(params) => {
                    let is_text = params.pointer("/update/sessionUpdate").and_then(serde_json::Value::as_str) == Some("agent_message_chunk");
                    if let (true, Some(chunk)) = (is_text, params.pointer("/update/content/text").and_then(serde_json::Value::as_str)) {
                        reply.push_str(chunk);
                    }
                }
                Incoming::Permission { id, params } => {
                    let title = params.pointer("/toolCall/title").and_then(serde_json::Value::as_str).unwrap_or("?");
                    trace(name, format!("debug: it asked permission: {title}"));
                    let _ = conn.reply(id, json!({ "outcome": { "outcome": "cancelled" } })).await;
                }
            }
        }
    };
    *conn.sink.lock().unwrap_or_else(PoisonError::into_inner) = None;
    outcome.map(|r| r.get("stopReason").and_then(serde_json::Value::as_str).unwrap_or("?").to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_output_is_cut_to_its_last_lines_without_npm_warnings() {
        let text = "npm warn deprecated x\nline 1\nline 2\nline 3";
        assert_eq!(last_lines(text, 2), "line 2\nline 3");
        assert_eq!(last_lines("", 3), "");
    }

    #[test]
    fn only_a_download_failure_counts_as_a_failed_setup() {
        assert!(fetch_failed("npm ERR! code E404"));
        assert!(!fetch_failed("agent: nothing to read"));
    }

    #[test]
    fn resetting_with_nothing_running_drops_nothing() {
        assert_eq!(Acp::new().reset(), 0);
    }
}
