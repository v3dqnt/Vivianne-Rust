//! Signing in to a coding agent before it is used. An agent that isn't signed in answers `session/new` with
//! `auth_required`, so that is how to tell: open a session on a fresh process and see. When it is not signed
//! in, the agent says how it can be: a method the agent runs itself (it opens a browser and waits), or a
//! command the user runs in a terminal.

use std::time::Duration;

use serde_json::{Value, json};

use super::conn::{Conn, RpcError, spawn, trace};
use super::registry::{Harness, find};
use super::session::{explain_start_failure, looks_unauthenticated};

/// Opening a probe session can wait on a first start.
const PROBE_TIMEOUT: Duration = Duration::from_mins(2);
/// A browser sign-in waits for the user.
const SIGN_IN_TIMEOUT: Duration = Duration::from_mins(5);

/// How an agent can be signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthKind {
    /// The agent runs it: it opens a browser or reads a key it already has.
    Agent,
    /// A command the user runs in a terminal.
    Terminal,
}

/// One way to sign an agent in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: AuthKind,
    /// For a terminal method: what to type.
    pub command: Option<String>,
}

/// Whether an agent can be used now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    SignedIn,
    /// It has to be signed in first, one of these ways.
    NeedsSignIn(Vec<AuthMethod>),
    /// It couldn't be asked.
    Failed(String),
}

/// What to type to sign in to an agent that gave no command of its own.
fn known_login_command(id: &str) -> Option<&'static str> {
    match id {
        "claude-acp" => Some("claude"),
        "codex-acp" => Some("codex login"),
        "gemini" => Some("gemini"),
        _ => None,
    }
}

/// The sign-in methods in an agent's answer to `initialize`. v2 names each `methodId` and says what kind it
/// is; v1 names them `id` and a missing kind means the agent runs it.
fn methods_in(caps: &Value, v2: bool, harness: &Harness) -> Vec<AuthMethod> {
    let listed = caps.get("authMethods").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut methods: Vec<AuthMethod> = listed
        .iter()
        .filter_map(|m| {
            let id = m.get(if v2 { "methodId" } else { "id" }).and_then(Value::as_str)?.to_owned();
            let kind = if m.get("type").and_then(Value::as_str) == Some("terminal") { AuthKind::Terminal } else { AuthKind::Agent };
            let text = |key: &str| m.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
            let command = (kind == AuthKind::Terminal).then(|| terminal_command(harness, m));
            Some(AuthMethod { name: if text("name").is_empty() { id.clone() } else { text("name") }, description: text("description"), id, kind, command })
        })
        .collect();
    // An agent that wants a sign-in and names no way to do it: the way it is known to take.
    if methods.is_empty()
        && let Some(command) = known_login_command(&harness.id)
    {
        methods.push(AuthMethod {
            id: "terminal".to_owned(),
            name: "Sign in in a terminal".to_owned(),
            description: String::new(),
            kind: AuthKind::Terminal,
            command: Some(command.to_owned()),
        });
    }
    methods
}

/// What to run in a terminal for a `terminal` method: the agent's own command with the method's arguments.
fn terminal_command(harness: &Harness, method: &Value) -> String {
    let args: Vec<String> = method.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
    if args.is_empty() {
        return known_login_command(&harness.id).map_or_else(|| format!("{} (sign in)", harness.name), str::to_owned);
    }
    let own = std::iter::once(harness.launcher.clone()).chain(harness.args_for_display()).chain(args);
    own.collect::<Vec<_>>().join(" ")
}

/// Opens a session on a connection to see whether the agent is signed in.
async fn probe(conn: &Conn, harness: &Harness, cwd: &str) -> AuthState {
    let opened = conn.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(PROBE_TIMEOUT)).await;
    match opened {
        Ok(session) => {
            // The probe is not a conversation: close it where the agent can.
            if let Some(id) = session.get("sessionId").and_then(Value::as_str)
                && conn.cap("/capabilities/session", "/agentCapabilities/sessionCapabilities/close")
            {
                let _ = conn.request("session/close", json!({ "sessionId": id }), Some(Duration::from_secs(10))).await;
            }
            AuthState::SignedIn
        }
        Err(e) if e.is_auth_required() || looks_unauthenticated(&e.message) => AuthState::NeedsSignIn(methods_in(&conn.caps(), conn.speaks_v2(), harness)),
        Err(e) => AuthState::Failed(explain_start_failure(harness, &e.to_string())),
    }
}

fn working_folder() -> String {
    std::env::current_dir().map_or_else(|_| ".".to_owned(), |d| d.display().to_string())
}

/// Whether the agent can be used now, asked of a fresh process.
pub async fn check(id: &str) -> Result<AuthState, String> {
    let harness = find(id).await?;
    if !harness.available {
        return Err(format!("{} isn't installed.", harness.name));
    }
    if harness.needs_setup() {
        return Err(format!("{} isn't set up yet.", harness.name));
    }
    let conn = spawn(&harness, &working_folder()).await?;
    let state = probe(&conn, &harness, &working_folder()).await;
    trace(&harness.name, format!("sign-in check: {state:?}"));
    Ok(state)
}

/// Signs the agent in with one of the methods it listed that the agent runs itself, then checks again. A
/// terminal method is not run here: the user runs it, then checks again.
pub async fn sign_in(id: &str, method_id: &str) -> Result<AuthState, String> {
    let harness = find(id).await?;
    if !harness.available || harness.needs_setup() {
        return Err(format!("{} isn't ready to sign in to.", harness.name));
    }
    let cwd = working_folder();
    let conn = spawn(&harness, &cwd).await?;
    let v2 = conn.speaks_v2();
    let known = methods_in(&conn.caps(), v2, &harness);
    let Some(method) = known.iter().find(|m| m.id == method_id) else {
        return Err(format!("{} doesn't offer that way to sign in.", harness.name));
    };
    if method.kind == AuthKind::Terminal {
        return Err(format!("That sign-in is run in a terminal: {}", method.command.clone().unwrap_or_default()));
    }
    let call = if v2 { "auth/login" } else { "authenticate" };
    trace(&harness.name, format!("signing in with method {method_id}"));
    conn.request(call, json!({ "methodId": method_id }), Some(SIGN_IN_TIMEOUT)).await.map_err(|e: RpcError| explain_start_failure(&harness, &e.to_string()))?;
    Ok(probe(&conn, &harness, &cwd).await)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::conn::testing::harness;
    use super::*;

    #[test]
    fn methods_are_read_in_each_versions_shape_with_terminal_ones_given_a_command() {
        let mut claude = harness("claude-acp");
        claude.name = "Claude Code".into();
        let v2 = json!({"authMethods": [
            {"methodId": "browser", "name": "Sign in with your browser", "type": "agent"},
            {"methodId": "term", "name": "Terminal", "type": "terminal"}
        ]});
        let methods = methods_in(&v2, true, &claude);
        assert_eq!((methods[0].id.as_str(), &methods[0].kind, methods[0].command.as_deref()), ("browser", &AuthKind::Agent, None));
        assert_eq!((&methods[1].kind, methods[1].command.as_deref()), (&AuthKind::Terminal, Some("claude")));
        let v1 = json!({"authMethods": [{"id": "oauth", "name": "OAuth"}]});
        let methods = methods_in(&v1, false, &claude);
        assert_eq!((methods[0].id.as_str(), &methods[0].kind), ("oauth", &AuthKind::Agent));
    }

    #[test]
    fn an_agent_that_names_no_way_gets_the_one_it_is_known_to_take() {
        let methods = methods_in(&json!({}), true, &harness("codex-acp"));
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].command.as_deref(), Some("codex login"));
        assert!(methods_in(&json!({}), true, &harness("unknown-agent")).is_empty());
    }

    #[tokio::test]
    async fn a_probe_that_opens_a_session_means_signed_in_and_one_that_wants_auth_lists_the_ways() {
        use super::super::conn::testing::peer;
        for signed_in in [true, false] {
            let (conn, mut agent) = peer("mock");
            let ready = tokio::spawn({
                let conn = conn.clone();
                async move { super::super::conn::handshake(&conn, &harness("claude-acp")).await }
            });
            agent.hello_version(2, json!({}), json!([{"methodId": "browser", "name": "Browser", "type": "agent"}])).await;
            assert!(ready.await.is_ok_and(|r| r.is_ok()));
            let asked = tokio::spawn({
                let conn = conn.clone();
                async move { probe(&conn, &harness("claude-acp"), "/p").await }
            });
            let request = agent.recv().await;
            assert_eq!(request["method"], "session/new");
            if signed_in {
                agent.answer(&request["id"], json!({"sessionId": "probe"})).await;
            } else {
                agent.refuse(&request["id"], -32000, "authentication required", Value::Null).await;
            }
            let state = asked.await.unwrap_or(AuthState::Failed(String::new()));
            if signed_in {
                assert_eq!(state, AuthState::SignedIn);
            } else {
                assert!(matches!(&state, AuthState::NeedsSignIn(m) if m.len() == 1 && m[0].id == "browser"), "{state:?}");
            }
        }
    }
}
