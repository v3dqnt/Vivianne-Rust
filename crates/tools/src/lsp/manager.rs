// Copied from Vivianne's original tools (0901 lsp/manager.rs).
// Kept as close to the original as possible; do not restyle.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, clippy::todo, clippy::dbg_macro, unsafe_code, missing_debug_implementations)]

//! Spawns and reuses one LSP client per language, project root and server configuration, lazily, the first time a file of that language is touched.
//! Best-effort: if no server for a language is on PATH, `touch_file` is a silent no-op.
//!
//! Language servers are heavy (rust-analyzer or tsserver can hold hundreds of MB to over a GB), and every agent edit starts one for post-edit
//! diagnostics. So they don't live for the whole session: a server idle for `IDLE_TIMEOUT` is shut down, and at most `MAX_SERVERS` run at once
//! (the least recently used goes first). The next edit simply starts it again. A server that fails to start is not tried again for
//! `START_RETRY`, so a broken install costs one attempt and not one per edit.
//!
//! Starting one is single-flight per key: the map lock is held only to find the slot, so a slow cold start of one language never blocks the others.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use super::client::LspClient;
use super::servers::{self, ServerDef};
use super::uri::{path_key, path_to_uri};
use super::Diagnostic;

const IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_SERVERS: usize = 3;
/// How long a failed start is remembered.
const START_RETRY: Duration = Duration::from_secs(3 * 60);
/// What a cold server start may add to an edit's wait before the edit's result goes back without it (the start carries on in the background).
const START_GRACE: Duration = Duration::from_secs(5);
const REFERENCES_RETRIES: usize = 2;
const REFERENCES_RETRY_DELAY: Duration = Duration::from_millis(250);
/// How long to wait for a server's project load before answering anyway.
const PROJECT_LOAD_WAIT: Duration = Duration::from_secs(15);

enum State {
    Empty,
    Up(Arc<LspClient>),
    /// The start failed at this time, for this reason.
    Failed(Instant, String),
}

struct Slot {
    state: Mutex<State>,
    last_used: StdMutex<Instant>,
    running: AtomicBool,
    language: String,
    root: String,
    line: String,
}

lazy_static::lazy_static! {
    /// Keyed by language, root, command line and configuration — a server is rooted at one project, and reusing it for another would diagnose
    /// files against the wrong tree; the same server started with other settings is a different server.
    static ref CLIENTS: StdMutex<HashMap<String, Arc<Slot>>> = StdMutex::new(HashMap::new());
}

fn extension_of(path: &str) -> Option<String> {
    Path::new(path).extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase())
}

fn config_digest(def: &ServerDef) -> u64 {
    // FNV-1a over the pieces that change what the server does.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in def.init_options.to_string().bytes().chain(def.settings.to_string().bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// The first server for `language_id` that is installed, with every definition tried and what was left out.
fn resolve_server(language_id: &str, workspace: &str) -> (Option<ServerDef>, servers::Candidates) {
    let c = servers::candidates(language_id, workspace);
    // A project's own programs (node_modules/.bin, .venv) count only once the user has trusted the project.
    let trusted = crate::host::project_trusted(workspace);
    let found = c.defs.iter().find_map(|d| {
        servers::resolve_command_with(&d.command, Path::new(workspace), trusted, &|p| p.is_file(), &|c| which::which(c).ok().map(|p| p.to_string_lossy().into_owned())).map(|command| ServerDef { command, ..d.clone() })
    });
    (found, c)
}

/// Get or lazily start the client for whichever language `path` belongs to.
async fn client_for(path: &str, workspace: &str) -> Result<Option<Arc<LspClient>>, String> {
    let Some(ext) = extension_of(path) else { return Ok(None) };
    let Some(language_id) = super::language_id_for_extension(&ext) else { return Ok(None) };
    let (def, cands) = resolve_server(language_id, workspace);
    for p in &cands.problems {
        log::warn!("lsp: {p}");
    }
    let Some(def) = def else { return Ok(None) };
    let root = servers::find_root(Path::new(path), workspace, &def.root_markers);
    let key = format!("{language_id}|{}|{}|{:016x}", path_key(&root), def.line(), config_digest(&def));

    ensure_reaper();
    let slot = {
        let mut map = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
        map.entry(key.clone())
            .or_insert_with(|| Arc::new(Slot { state: Mutex::new(State::Empty), last_used: StdMutex::new(Instant::now()), running: AtomicBool::new(false), language: language_id.to_string(), root: root.clone(), line: def.line() }))
            .clone()
    };
    *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();

    let mut guard = slot.state.lock().await;
    match &*guard {
        State::Up(c) if !c.is_closed() => return Ok(Some(c.clone())),
        State::Failed(at, why) if at.elapsed() < START_RETRY => return Err(format!("{} did not start {}s ago ({why}); not retried for {} minutes", def.command, at.elapsed().as_secs(), START_RETRY.as_secs() / 60)),
        _ => {}
    }
    let args: Vec<&str> = def.args.iter().map(|a| a.as_str()).collect();
    match LspClient::spawn(&def.command, &args, &root, &def.init_options, &def.settings).await {
        Ok(client) => {
            *guard = State::Up(client.clone());
            slot.running.store(true, Ordering::SeqCst);
            drop(guard);
            evict_over_cap(&key);
            Ok(Some(client))
        }
        Err(e) => {
            log::warn!("lsp: failed to start {} for {language_id}: {e}", def.command);
            // A start that merely ran out of time may work next time; only a start that failed is remembered.
            *guard = if e.contains("timed out") { State::Empty } else { State::Failed(Instant::now(), e.clone()) };
            slot.running.store(false, Ordering::SeqCst);
            Err(format!("{} did not start: {e}", def.command))
        }
    }
}

/// A started client, or why there isn't one.
pub async fn client(path: &str, workspace: &str) -> Result<Arc<LspClient>, String> {
    match client_for(path, workspace).await? {
        Some(c) => Ok(c),
        None => Err(no_server_message(path, workspace)),
    }
}

fn no_server_message(path: &str, workspace: &str) -> String {
    let Some(language_id) = extension_of(path).and_then(|e| super::language_id_for_extension(&e)) else {
        return "no language server handles this file type".into();
    };
    let c = servers::candidates(language_id, workspace);
    let tried: Vec<String> = c.defs.iter().map(|d| d.command.clone()).collect();
    let mut msg = if tried.is_empty() { format!("no language server is known for {language_id}") } else { format!("no language server for {language_id} is installed (tried {})", tried.join(", ")) };
    if !c.ignored.is_empty() {
        msg.push_str(&format!("; this project defines {} but it is ignored until the project is trusted", c.ignored.join(" and ")));
    }
    msg
}

/// Shut down the server a slot holds, if it holds one.
async fn stop_slot(slot: &Slot) {
    let mut state = slot.state.lock().await;
    if let State::Up(c) = std::mem::replace(&mut *state, State::Empty) {
        c.shutdown().await;
    }
    slot.running.store(false, Ordering::SeqCst);
}

/// Stop the least recently used running server(s) beyond `MAX_SERVERS`, never the one just started.
fn evict_over_cap(just_started: &str) {
    let mut victims: Vec<Arc<Slot>> = Vec::new();
    {
        let mut map = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let running: Vec<(String, Instant)> = map.iter().filter(|(k, s)| s.running.load(Ordering::SeqCst) && k.as_str() != just_started).map(|(k, s)| (k.clone(), *s.last_used.lock().unwrap_or_else(|e| e.into_inner()))).collect();
            if running.len() + 1 <= MAX_SERVERS {
                break;
            }
            let Some((oldest, _)) = running.into_iter().min_by_key(|(_, t)| *t) else { break };
            if let Some(s) = map.remove(&oldest) {
                log::info!("lsp: stopping {oldest:?} to stay within {MAX_SERVERS} servers");
                victims.push(s);
            }
        }
    }
    for s in victims {
        tokio::spawn(async move { stop_slot(&s).await });
    }
}

/// Starts (once) the background task that shuts down idle servers.
fn ensure_reaper() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let stale: Vec<Arc<Slot>> = {
                    let mut map = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
                    // Quiet for the window AND nothing waiting on it: a long request must never look idle. A slot being started is busy.
                    let keys: Vec<String> = map
                        .iter()
                        .filter(|(_, s)| {
                            let quiet = s.last_used.lock().unwrap_or_else(|e| e.into_inner()).elapsed() >= IDLE_TIMEOUT;
                            quiet
                                && match s.state.try_lock() {
                                    Ok(state) => match &*state {
                                        State::Up(c) => c.is_idle(IDLE_TIMEOUT),
                                        _ => true,
                                    },
                                    Err(_) => false,
                                }
                        })
                        .map(|(k, _)| k.clone())
                        .collect();
                    keys.into_iter().filter_map(|k| map.remove(&k)).collect()
                };
                for s in stale {
                    log::info!("lsp: stopping an idle language server");
                    stop_slot(&s).await;
                }
            }
        });
    });
}

/// What a language server said about one file.
pub enum Report {
    /// No server is installed for this file type.
    NoServer,
    /// A server is running but hadn't published for this file within the wait.
    NotReported,
    /// The server's diagnostics (possibly none).
    Published(Vec<Diagnostic>),
}

/// What an edit's check found, and, when the server hadn't answered, what to wait on afterwards.
struct Outcome {
    report: Report,
}

async fn check(path: &str, root_dir: &str, wait: Duration) -> Outcome {
    // The start runs as its own task: if it outlasts this edit's wait the edit goes on without it, and the server is there for the next one.
    let start = {
        let (p, r) = (path.to_string(), root_dir.to_string());
        tokio::spawn(async move { client_for(&p, &r).await })
    };
    let client = match tokio::time::timeout(wait + START_GRACE, start).await {
        Ok(Ok(Ok(Some(c)))) => c,
        Ok(Ok(Ok(None))) | Ok(Err(_)) => return Outcome { report: Report::NoServer },
        // A server that is installed but would not start is not a file without problems.
        Ok(Ok(Err(e))) => {
            log::warn!("lsp: {e}");
            return Outcome { report: Report::NoServer };
        }
        Err(_) => return Outcome { report: Report::NotReported },
    };
    // What the server said before this edit is not an answer about it: only a report for the text just sent counts.
    let mark = match client.touch_file(path).await {
        Ok(mark) => mark,
        Err(e) => {
            log::warn!("lsp: touch_file failed for {path}: {e}");
            return Outcome { report: Report::NotReported };
        }
    };
    client.did_save(path).await;
    match client.fresh_report(path, &mark, wait).await {
        Some(items) => Outcome { report: Report::Published(items) },
        None => Outcome { report: Report::NotReported },
    }
}

/// Like `diagnostics_for`, but tells "no server", "no answer yet" and "no problems" apart, and waits up to `wait` for the first report.
pub async fn report(path: &str, root_dir: &str, wait: Duration) -> Report {
    check(path, root_dir, wait).await.report
}

/// A navigation query (`goToDefinition`, `findReferences`, `goToImplementation`, `goToTypeDefinition`, `hover`) at a 1-based `line` and character `col`.
pub async fn query(op: &str, path: &str, root_dir: &str, line: u32, col: u32) -> Result<Value, String> {
    let c = client(path, root_dir).await?;
    let mut result = c.query(op, path, line, col).await?;
    if op == "findReferences" {
        // A server still loading its project answers with nothing, or only the declaration it was asked about: wait for it to finish, ask again.
        for _ in 0..REFERENCES_RETRIES {
            if result.as_array().map(|a| a.len()).unwrap_or(0) > 1 {
                break;
            }
            c.wait_until_loaded(PROJECT_LOAD_WAIT).await;
            tokio::time::sleep(REFERENCES_RETRY_DELAY).await;
            result = c.query(op, path, line, col).await?;
        }
    }
    Ok(result)
}

/// The symbols of one file (a tree or a flat list, as the server sends them), or, with `query` and no path, the workspace's.
pub async fn symbols(path: &str, root_dir: &str, query: Option<&str>) -> Result<Value, String> {
    let c = client(path, root_dir).await?;
    match query {
        Some(q) if !q.is_empty() => {
            if !c.has("workspaceSymbolProvider").await {
                return Err("this language server does not search workspace symbols".into());
            }
            c.call("workspace/symbol", json!({ "query": q })).await
        }
        _ => {
            if !c.has("documentSymbolProvider").await {
                return Err("this language server does not list a file's symbols".into());
            }
            c.touch_file(path).await?;
            c.call("textDocument/documentSymbol", json!({ "textDocument": { "uri": path_to_uri(path) } })).await
        }
    }
}

/// What is running, what failed, and what a project defines that is not in use.
pub async fn status(workspace: &str) -> String {
    let slots: Vec<Arc<Slot>> = CLIENTS.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
    let mut lines = Vec::new();
    for s in slots {
        let age = s.last_used.lock().unwrap_or_else(|e| e.into_inner()).elapsed().as_secs();
        match &*s.state.lock().await {
            State::Up(c) if !c.is_closed() => lines.push(format!("running  {} ({}) at {} — {} file(s) open, used {age}s ago", s.language, s.line, s.root, c.open_files().await.len())),
            State::Up(_) => lines.push(format!("exited   {} ({}) at {}", s.language, s.line, s.root)),
            State::Failed(at, why) => lines.push(format!("failed   {} ({}) at {} — {why}; retried after {}s", s.language, s.line, s.root, START_RETRY.as_secs().saturating_sub(at.elapsed().as_secs()))),
            State::Empty => {}
        }
    }
    lines.sort();
    if lines.is_empty() {
        lines.push("no language server is running".into());
    }
    if let Ok(text) = std::fs::read_to_string(Path::new(workspace).join(".viv").join("lsp.json")) {
        let trusted = crate::host::project_trusted(workspace);
        let defined: Vec<String> = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("servers").and_then(|s| s.as_object()).cloned()).map(|m| m.keys().flat_map(|lang| servers::from_config(&text, lang, servers::Source::Project).unwrap_or_default().into_iter().map(move |d| format!("{lang}: {}", d.line()))).collect()).unwrap_or_default();
        if !defined.is_empty() {
            lines.push(if trusted { format!("project definitions in use: {}", defined.join("; ")) } else { format!("project definitions IGNORED until the project is trusted: {}", defined.join("; ")) });
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_with_other_settings_is_another_server() {
        let mut a = ServerDef { command: "x".into(), args: vec![], root_markers: vec![], init_options: Value::Null, settings: Value::Null, source: servers::Source::User };
        let b = a.clone();
        a.settings = json!({ "strict": true });
        assert_ne!(config_digest(&a), config_digest(&b));
        a.settings = Value::Null;
        a.init_options = json!({ "x": 1 });
        assert_ne!(config_digest(&a), config_digest(&b));
        assert_eq!(config_digest(&b), config_digest(&b.clone()));
    }

}
