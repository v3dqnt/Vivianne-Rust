//! A thread's session with an agent: opening it (new, or reopened after a restart), signing the agent in when
//! it asks, remembering it, and telling it what it missed of the conversation.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use vivianne_ai::paths::{viv_home_path, write_json_atomic};

use super::conn::{Conn, RpcError, trace};
use super::info::forget_context;
use super::registry::Harness;

/// The thread id a delegated job runs under: never saved, never resumed.
pub(super) const DELEGATE: &str = "delegate-";
/// Opening or reopening a session can wait on a first download.
const SESSION_TIMEOUT: Duration = Duration::from_mins(2);
const SIGN_IN_TIMEOUT: Duration = Duration::from_mins(2);
/// How much of what was said before the agent joined is handed to it.
const DIGEST_CHARS: usize = 8000;

/// One thing said in the conversation before this turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `User` or `Assistant`.
    pub who: &'static str,
    pub text: String,
}

// ── Saved sessions ──────────────────────────────────────────────────────────

pub(super) fn session_key(thread: &str, harness: &str) -> String {
    format!("{thread}|{harness}")
}

fn sessions_path() -> std::path::PathBuf {
    viv_home_path(&["acp-sessions.json"])
}

static SESSIONS_WRITE: Mutex<()> = Mutex::new(());

/// Each thread's session with each agent, and how much of the thread it had seen: read after a restart to
/// reopen it.
pub(super) fn saved_sessions() -> HashMap<String, (String, usize)> {
    std::fs::read_to_string(sessions_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

pub(super) fn save_session(key: &str, session: &str, seen: usize) {
    let _guard = SESSIONS_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut all = saved_sessions();
    all.insert(key.to_owned(), (session.to_owned(), seen));
    let _ = write_json_atomic(&sessions_path(), &all);
}

/// Forgets a saved session the agent has closed, so the next turn starts a new one. Only when the entry still
/// names it: a turn that already saved a newer session must keep that one.
pub(super) fn forget_session(key: &str, session: &str) {
    let _guard = SESSIONS_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut all = saved_sessions();
    if all.get(key).is_none_or(|(saved, _)| saved != session) {
        return;
    }
    all.remove(key);
    let _ = write_json_atomic(&sessions_path(), &all);
}

// ── What it missed ──────────────────────────────────────────────────────────

/// What the agent's session hasn't seen of the thread, as one block of text put before the user's message: the
/// newest `DIGEST_CHARS` of what was said. Tool calls are left out; what they changed is on disk for it to read.
pub(super) fn digest(missed: &[Line]) -> Option<String> {
    let lines: Vec<String> = missed.iter().filter(|l| !l.text.trim().is_empty()).map(|l| format!("{}: {}", l.who, l.text.trim())).collect();
    if lines.is_empty() {
        return None;
    }
    let all = lines.join("\n\n");
    let skip = all.chars().count().saturating_sub(DIGEST_CHARS);
    let tail: String = all.chars().skip(skip).collect();
    Some(format!(
        "<earlier_conversation>\nThis thread was already under way before you joined it. What was said{}:\n\n{tail}\n</earlier_conversation>\n\nThat is background, not a request. The user's message follows.",
        if skip > 0 { " (the newest part)" } else { "" }
    ))
}

// ── Failures that mean "sign in" ────────────────────────────────────────────

/// Whether an agent's error is its way of saying it isn't signed in.
pub(super) fn looks_unauthenticated(error: &str) -> bool {
    let low = error.to_lowercase();
    [
        "authentication required",
        "not authenticated",
        "unauthenticated",
        "oauth session expired",
        "please log in",
        "please login",
        "invalid api key",
        "failed to authenticate",
    ]
    .iter()
    .any(|m| low.contains(m))
}

/// What the user can do when a session won't open: usually sign in.
pub(super) fn explain_start_failure(harness: &Harness, error: &str) -> String {
    let low = error.to_lowercase();
    if ["auth", "login", "log in", "sign in", "api key"].iter().any(|word| low.contains(word)) {
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

/// The sign-in method Vivianne can run, if the agent offers one. v2 requires every method to say what kind it
/// is and names them `methodId`; v1 leaves `type` optional (a missing one means `agent`) and names them `id`.
/// A `terminal` method is not something a client runs on the agent's behalf.
pub(super) fn auth_method(caps: &Value, v2: bool) -> Option<String> {
    caps.get("authMethods")?.as_array()?.iter().find_map(|m| {
        let kind = m.get("type").and_then(Value::as_str);
        let agent = if v2 { kind == Some("agent") } else { kind.is_none() || kind == Some("agent") };
        if !agent {
            return None;
        }
        m.get(if v2 { "methodId" } else { "id" }).and_then(Value::as_str).map(str::to_owned)
    })
}

/// Signs the agent in with a method it offers. When it offers only a terminal method, says what to run by hand
/// instead of failing on a code.
async fn sign_in(conn: &Conn, harness: &Harness) -> Result<(), String> {
    let v2 = conn.speaks_v2();
    let Some(method) = auth_method(&conn.caps(), v2) else {
        return Err(explain_start_failure(
            harness,
            "it wants you to sign in first, and offers no sign-in Vivianne can run (a terminal sign-in has to be done by hand)",
        ));
    };
    let call = if v2 { "auth/login" } else { "authenticate" };
    trace(&harness.name, format!("signing in with method {method}"));
    conn.request(call, json!({ "methodId": method }), Some(SIGN_IN_TIMEOUT)).await.map_err(|e| explain_start_failure(harness, &e.to_string()))?;
    Ok(())
}

// ── Opening a session ───────────────────────────────────────────────────────

/// How this agent reopens a session, if it can. v2 resumes; v1 loads (and replays history) when it advertises
/// `loadSession`, else resumes.
fn resume_method(conn: &Conn) -> Option<&'static str> {
    if conn.speaks_v2() {
        return conn.flag("/capabilities/session").then_some("session/resume");
    }
    if conn.flag("/agentCapabilities/loadSession") {
        Some("session/load")
    } else if conn.flag("/agentCapabilities/sessionCapabilities/resume") {
        Some("session/resume")
    } else {
        None
    }
}

/// Opens the thread's session: resumes the saved one when the agent can, else starts a new one. Returns the
/// session id and how much of the thread it has already seen.
async fn setup_session(conn: &Conn, saved: Option<(String, usize)>, cwd: &str, servers: &[Value]) -> Result<(String, usize), RpcError> {
    if let Some((id, seen)) = saved {
        if let Some(method) = resume_method(conn) {
            // No `replayFrom` on a v2 resume: Vivianne doesn't re-render history, and the turn's digest
            // covers what the session missed.
            match conn.request(method, json!({ "sessionId": id, "cwd": cwd, "mcpServers": servers }), Some(SESSION_TIMEOUT)).await {
                Ok(_) => {
                    trace("harness", format!("reopened session {id} with {method}"));
                    return Ok((id, seen));
                }
                // The agent no longer has it, or refused for some other reason: a new session is better than
                // a stuck thread, and the digest fills in what it missed.
                Err(e) => trace("harness", format!("{method} for {id} failed ({e}); starting a new session")),
            }
        } else {
            trace("harness", format!("it can't reopen session {id}; starting a new session"));
        }
    }
    let opened = conn.request("session/new", json!({ "cwd": cwd, "mcpServers": servers }), Some(SESSION_TIMEOUT)).await?;
    let id = opened
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::transport("the agent opened a session without a session id"))?
        .to_owned();
    Ok((id, 0))
}

/// The thread's session on the agent: reopened after a restart when the agent can, otherwise new.
pub(super) async fn open_session(conn: &Conn, harness: &Harness, thread: &str, cwd: &str, servers: &[Value]) -> Result<String, String> {
    let saved = saved_sessions().remove(&session_key(thread, &harness.id));
    let id = open_with(conn, harness, saved, cwd, servers).await?;
    // A session that has seen nothing is a new one: what the last said about how full it was no longer holds.
    if conn.seen.load(Ordering::SeqCst) == 0 {
        forget_context(thread);
    }
    Ok(id)
}

/// The rest of [`open_session`], with the saved session handed in so a test doesn't have to write the
/// saved-sessions file.
pub(super) async fn open_with(conn: &Conn, harness: &Harness, saved: Option<(String, usize)>, cwd: &str, servers: &[Value]) -> Result<String, String> {
    let mut session = conn.session.lock().await;
    if session.is_none() {
        let mut setup = setup_session(conn, saved, cwd, servers).await;
        // An agent that wants a sign-in first says so once, then goes on.
        if matches!(&setup, Err(e) if e.is_auth_required()) {
            sign_in(conn, harness).await?;
            setup = setup_session(conn, None, cwd, servers).await;
        }
        let (id, seen) = setup.map_err(|e| explain_start_failure(harness, &e.to_string()))?;
        conn.seen.store(seen, Ordering::SeqCst);
        *session = Some(id);
    }
    session.clone().ok_or_else(|| "the agent opened no session".to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::conn::testing::{harness, peer};
    use super::*;

    fn line(who: &'static str, text: &str) -> Line {
        Line { who, text: text.to_owned() }
    }

    #[test]
    fn the_digest_gives_the_agent_what_it_missed_and_cuts_it_to_the_newest() {
        assert_eq!(digest(&[]), None);
        assert_eq!(digest(&[line("User", "  ")]), None);
        let short = digest(&[line("User", "hi"), line("Assistant", "hello")]).unwrap_or_default();
        assert!(short.contains("User: hi\n\nAssistant: hello") && !short.contains("newest part"));
        let long = digest(&[line("User", &"x".repeat(DIGEST_CHARS * 2))]).unwrap_or_default();
        assert!(long.contains("(the newest part)") && long.chars().count() < DIGEST_CHARS + 400);
    }

    #[test]
    fn an_agents_sign_in_errors_are_recognised() {
        for e in ["Authentication required", "Failed to authenticate: OAuth session expired", "401 Invalid API key"] {
            assert!(looks_unauthenticated(e), "{e}");
        }
        assert!(!looks_unauthenticated("the agent exited") && !looks_unauthenticated("rate limit reached"));
    }

    #[test]
    fn the_sign_in_method_is_read_in_each_versions_shape_and_a_terminal_one_is_not_run() {
        let v2 = json!({"authMethods": [{"type": "terminal", "methodId": "t"}, {"type": "agent", "methodId": "a"}]});
        assert_eq!(auth_method(&v2, true).as_deref(), Some("a"));
        let v1 = json!({"authMethods": [{"id": "oauth"}]});
        assert_eq!(auth_method(&v1, false).as_deref(), Some("oauth"));
        assert_eq!(auth_method(&json!({"authMethods": [{"type": "terminal", "methodId": "t"}]}), true), None);
    }

    async fn start(version: u64, caps: Value, auth: Value) -> (std::sync::Arc<Conn>, super::super::conn::testing::Peer) {
        let (conn, mut agent) = peer("mock");
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { super::super::conn::handshake(&conn, &harness("a")).await }
        });
        agent.hello_version(version, caps, auth).await;
        assert!(asked.await.is_ok_and(|r| r.is_ok()));
        (conn, agent)
    }

    #[tokio::test]
    async fn a_new_thread_gets_a_new_session() {
        let (conn, mut agent) = start(2, json!({}), json!([])).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("a"), None, "/p", &[]).await }
        });
        let asked = agent.open("s1").await;
        assert_eq!((asked["params"]["cwd"].as_str(), asked["params"]["mcpServers"].as_array().map(Vec::len)), (Some("/p"), Some(0)));
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("s1".to_owned()));
    }

    #[tokio::test]
    async fn a_v2_agent_reopens_the_saved_session_without_replaying_it() {
        let (conn, mut agent) = start(2, json!({"session": {}}), json!([])).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("a"), Some(("old".into(), 4)), "/p", &[]).await }
        });
        let msg = agent.recv().await;
        assert_eq!((msg["method"].as_str(), msg["params"]["sessionId"].as_str()), (Some("session/resume"), Some("old")));
        agent.answer(&msg["id"], json!({})).await;
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("old".to_owned()));
        assert_eq!(conn.seen.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn a_resume_the_agent_no_longer_has_starts_a_new_session() {
        let (conn, mut agent) = start(2, json!({"session": {}}), json!([])).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("a"), Some(("gone".into(), 4)), "/p", &[]).await }
        });
        let msg = agent.recv().await;
        agent.refuse(&msg["id"], -32002, "no such session", Value::Null).await;
        agent.open("fresh").await;
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("fresh".to_owned()));
        assert_eq!(conn.seen.load(Ordering::SeqCst), 0, "a new session has seen nothing");
    }

    #[tokio::test]
    async fn a_v1_agent_that_can_load_reopens_with_session_load() {
        let (conn, mut agent) = start(1, json!({"loadSession": true}), json!([])).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("a"), Some(("old".into(), 2)), "/p", &[]).await }
        });
        let msg = agent.recv().await;
        assert_eq!(msg["method"], "session/load");
        agent.answer(&msg["id"], json!({})).await;
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("old".to_owned()));
    }

    #[tokio::test]
    async fn an_agent_that_cannot_reopen_sessions_gets_a_new_one() {
        let (conn, mut agent) = start(1, json!({}), json!([])).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("a"), Some(("old".into(), 2)), "/p", &[]).await }
        });
        agent.open("new").await;
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("new".to_owned()));
    }

    #[tokio::test]
    async fn a_v2_agent_that_wants_a_sign_in_gets_one_then_the_setup_is_retried() {
        let auth = json!([{"type": "agent", "methodId": "login"}]);
        let (conn, mut agent) = start(2, json!({}), auth).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("claude-acp"), None, "/p", &[]).await }
        });
        let first = agent.recv().await;
        assert_eq!(first["method"], "session/new");
        agent.refuse(&first["id"], -32000, "auth required", Value::Null).await;
        let login = agent.recv().await;
        assert_eq!((login["method"].as_str(), login["params"]["methodId"].as_str()), (Some("auth/login"), Some("login")));
        agent.answer(&login["id"], json!({})).await;
        agent.open("s9").await;
        assert_eq!(opened.await.unwrap_or(Err(String::new())), Ok("s9".to_owned()));
    }

    #[tokio::test]
    async fn an_agent_that_offers_only_a_terminal_sign_in_says_what_to_do() {
        let auth = json!([{"type": "terminal", "methodId": "t"}]);
        let (conn, mut agent) = start(2, json!({}), auth).await;
        let opened = tokio::spawn({
            let conn = conn.clone();
            async move { open_with(&conn, &harness("claude-acp"), None, "/p", &[]).await }
        });
        let first = agent.recv().await;
        agent.refuse(&first["id"], -32000, "auth required", Value::Null).await;
        let failure = opened.await.unwrap_or(Ok(String::new())).err().unwrap_or_default();
        assert!(failure.contains("needs you to sign in") && failure.contains("claude"), "{failure}");
        assert!(failure.contains("terminal sign-in has to be done by hand"), "{failure}");
    }
}
