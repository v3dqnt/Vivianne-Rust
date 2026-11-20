//! Other coding agents (Claude Code, Codex, Gemini CLI and the rest of the Agent Client Protocol registry) as
//! something a thread can talk to.
//!
//! The agent runs as a child process speaking ACP (JSON-RPC 2.0, one message per line over stdio): `initialize`
//! (protocol v2, falling back to v1), `session/new` or a resume once per thread, then `session/prompt` per turn
//! while it streams `session/update` notifications back. [`Acp::run_turn`] turns those into [`AcpEvent`]s.
//!
//! - `registry.rs`: which agents exist and how to start them (and npx's cache, so an agent already fetched
//!   starts straight from it).
//! - `install.rs`: installing an agent under `~/.viv/agents` so it starts with no `npx` and no network.
//! - `auth.rs`: telling whether an agent is signed in, and signing it in, before it is used.
//! - `conn.rs`: a running agent, the handshake and the trace of its traffic.
//! - `session.rs`: opening, reopening and signing in to a session; telling the agent what it missed.
//! - `turn.rs`: one turn, how it ends, and stopping it.
//! - `permission.rs`: what the agent may do without asking.
//! - `info.rs`: what the agent says about its session; `stderr.rs`: what it said on the way out.

mod auth;
mod conn;
mod info;
mod install;
mod permission;
mod registry;
mod session;
mod stderr;
mod turn;

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use conn::{Conn, quiet, spawn, trace};
pub use auth::{AuthKind, AuthMethod, AuthState, Depth, check as check_auth, options as sign_in_options, sign_in};
pub use conn::trace_lines;
pub use info::{AgentCommand, SessionInfo, context_usage};
pub use install::InstallProgress;
pub use registry::{Harness, find, harnesses, is_usable};
pub use session::Line;
pub use turn::{AcpEvent, Outcome, PermissionRequest, PromptPart, ToolInfo, TurnEnd, TurnRequest};

/// The `provider` a model served by an agent carries.
pub const PROVIDER: &str = "harness";

const SETUP_LIMIT: Duration = Duration::from_mins(5);
/// How long a test prompt may take.
const DEBUG_PROMPT_LIMIT: Duration = Duration::from_secs(90);

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
    /// Stopping through `cancel` asks the agent to stop. An `Err` is a turn that never got going (the agent
    /// isn't installed, isn't set up, or wouldn't start); a turn that began and failed is
    /// [`Outcome::Failed`].
    pub async fn run_turn(
        &self,
        thread: &str,
        harness_id: &str,
        cwd: &str,
        request: &TurnRequest<'_>,
        events: &mpsc::UnboundedSender<AcpEvent>,
        cancel: &CancellationToken,
    ) -> Result<TurnEnd, String> {
        let harness = find(harness_id).await?;
        if !harness.available {
            return Err(format!(
                "{} runs through {}, which isn't installed. Install Node.js (for npx) or uv (for uvx) and try again.",
                harness.name,
                harness.launcher_name()
            ));
        }
        if harness.needs_setup() {
            return Err(format!(
                "{} isn't set up yet. Set it up first (Settings, Providers, Coding agents): it downloads the agent once, which can be a large download.",
                harness.name
            ));
        }
        trace(&harness.name, format!("turn: connecting (thread {thread})"));
        let conn = self.connection(thread, &harness, cwd).await?;
        let session = session::open_session(&conn, &harness, thread, cwd).await?;
        let end = turn::run(&conn, &harness, &session, thread, request, events, cancel).await;
        if !conn.is_alive() {
            // The next turn starts a fresh agent.
            self.conns.lock().unwrap_or_else(PoisonError::into_inner).remove(&(thread.to_owned(), harness.id.clone()));
        }
        Ok(end)
    }

    async fn connection(&self, thread: &str, harness: &Harness, cwd: &str) -> Result<Arc<Conn>, String> {
        let key = (thread.to_owned(), harness.id.clone());
        let existing = self.conns.lock().unwrap_or_else(PoisonError::into_inner).get(&key).filter(|c| c.is_alive()).cloned();
        if let Some(conn) = existing {
            return Ok(conn);
        }
        let conn = spawn(harness, cwd).await?;
        self.conns.lock().unwrap_or_else(PoisonError::into_inner).insert(key, conn.clone());
        Ok(conn)
    }

    /// What the agent has said about this thread's session with it (its commands, mode, options and title), if
    /// one is running.
    #[must_use]
    pub fn session_info(&self, thread: &str, harness_id: &str) -> Option<SessionInfo> {
        let conn = self.conns.lock().unwrap_or_else(PoisonError::into_inner).get(&(thread.to_owned(), harness_id.to_owned())).cloned()?;
        let info = conn.info.lock().unwrap_or_else(PoisonError::into_inner).clone();
        Some(info)
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

/// Sets an agent up. An npm agent is installed under `~/.viv/agents` (so it starts with no `npx` and no
/// network), telling `progress` what npm is doing; any other agent's launcher is run once with no input, which
/// fetches the package into the launcher's cache. After that, a turn starts at once.
pub async fn install(id: &str, progress: &(dyn Fn(InstallProgress) + Sync)) -> Result<String, String> {
    let harness = find(id).await?;
    if !harness.available {
        return Err(format!(
            "{} runs through {}, which isn't installed. Install Node.js (for npx) or uv (for uvx) first.",
            harness.name,
            harness.launcher_name()
        ));
    }
    if let Some(package) = harness.package() {
        return install::install_managed(&harness, package, progress).await;
    }
    let program = which::which(&harness.launcher).map_err(|_| format!("{} isn't installed.", harness.launcher))?;
    let mut command = quiet(&program);
    command.args(&harness.args).envs(&harness.env).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    trace(&harness.name, format!("setting up: {} {}", program.display(), harness.args.join(" ")));
    let child = command.spawn().map_err(|e| format!("Couldn't run {}: {e}", program.display()))?;
    let output = tokio::time::timeout(SETUP_LIMIT, child.wait_with_output())
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
/// start and `initialize`, open a session, then one short prompt. Nothing here touches a thread.
pub async fn debug(id: &str, prompt: Option<String>) -> Result<DebugReport, String> {
    let harness = find(id).await?;
    let mut report = DebugReport::default();
    let started = Instant::now();
    report.steps.push(step(
        "Launcher found",
        harness.available,
        started,
        if harness.available { format!("{} {}", harness.launcher, harness.args.join(" ")) } else { format!("{} isn't installed", harness.launcher_name()) },
    ));
    if harness.available && harness.package().is_some() {
        let installed = !harness.needs_setup();
        let detail = if harness.managed {
            "installed under ~/.viv/agents"
        } else if harness.ready {
            "started through npx before"
        } else {
            "not installed: use Set up first (it downloads the agent once)"
        };
        report.steps.push(step("Installed", installed, started, detail.to_owned()));
        if !installed {
            report.trace = trace_lines();
            return Ok(report);
        }
    }
    if harness.available {
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
            report.steps.push(step("Start and initialize", true, t, format!("protocol version {}", conn.version())));
            conn
        }
        Err(why) => {
            report.steps.push(step("Start and initialize", false, t, why));
            return;
        }
    };

    let t = Instant::now();
    let thread = format!("{}debug-{}", session::DELEGATE, harness.id);
    let session = match session::open_with(&conn, harness, None, &cwd).await {
        Ok(session) => {
            report.steps.push(step("Open a session", true, t, format!("session {session}")));
            session
        }
        Err(why) => {
            report.steps.push(step("Open a session", false, t, why));
            return;
        }
    };

    // One short prompt through the real turn loop, with a time limit. A permission asked here is a failure to
    // report, not a thing to grant.
    let t = Instant::now();
    let (tx, mut events) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let limit = cancel.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(DEBUG_PROMPT_LIMIT).await;
        limit.cancel();
    });
    let reader = tokio::spawn(async move {
        let mut reply = String::new();
        while let Some(event) = events.recv().await {
            match event {
                AcpEvent::Text(piece) => reply.push_str(&piece),
                AcpEvent::Permission(request) => {
                    trace("debug", format!("it asked permission: {}", request.why.clone().unwrap_or_default()));
                    request.answer(false);
                }
                _ => {}
            }
        }
        reply
    });
    let text = prompt.unwrap_or_else(|| "Reply with the single word: ok".to_owned());
    let parts = [PromptPart::Text(text)];
    let request = TurnRequest { prompt: &parts, earlier: &[], plan_mode: false, bypass: false };
    let end = turn::run(&conn, harness, &session, &thread, &request, &tx, &cancel).await;
    timer.abort();
    drop(tx);
    report.reply = reader.await.unwrap_or_default();
    match end.outcome {
        Outcome::Finished => report.steps.push(step("Answer a prompt", true, t, "it answered".to_owned())),
        Outcome::Stopped => report.steps.push(step("Answer a prompt", false, t, "no answer in 90s".to_owned())),
        Outcome::Failed(why) => report.steps.push(step("Answer a prompt", false, t, why)),
    }
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
        assert!(Acp::new().session_info("t", "a").is_none());
    }

    #[test]
    fn the_providers_own_name_is_harness() {
        assert_eq!(PROVIDER, "harness");
        let _ = json!({});
    }
}
