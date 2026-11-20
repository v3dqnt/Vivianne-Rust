//! Signing in to a coding agent before it is used. An agent that isn't signed in answers `session/new` with
//! `auth_required`, so that is how to tell: open a session on a fresh process and see. When it is not signed
//! in, the agent says how it can be: a method the agent runs itself (it opens a browser and waits), or a
//! command the user runs in a terminal.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::conn::{Conn, RpcError, spawn, trace};
use super::registry::{Harness, find};
use super::session::{DELEGATE, explain_start_failure, looks_unauthenticated};
use super::turn::{self, AcpEvent, Outcome, PromptPart, TurnRequest};

/// Opening a probe session can wait on a first start.
const PROBE_TIMEOUT: Duration = Duration::from_mins(2);
/// How long the agent gets to answer the check question.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(90);
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

/// Opens a session on a connection. An agent that is plainly not signed in answers `auth_required` here; one
/// that checks only when asked (Claude's adapter does) opens a session fine, so this alone proves nothing.
async fn open_probe(conn: &Conn, harness: &Harness, cwd: &str) -> Result<String, AuthState> {
    let opened = conn.request("session/new", json!({ "cwd": cwd, "mcpServers": [] }), Some(PROBE_TIMEOUT)).await;
    match opened {
        Ok(session) => session.get("sessionId").and_then(Value::as_str).map(str::to_owned).ok_or_else(|| AuthState::Failed(format!("{} opened a session without an id.", harness.name))),
        Err(e) if e.is_auth_required() || looks_unauthenticated(&e.message) => Err(AuthState::NeedsSignIn(methods_in(&conn.caps(), conn.speaks_v2(), harness))),
        Err(e) => Err(AuthState::Failed(explain_start_failure(harness, &e.to_string()))),
    }
}

/// Whether a failure message is the agent saying it isn't signed in.
fn says_sign_in(message: &str) -> bool {
    looks_unauthenticated(message) || message.contains("needs you to sign in")
}

/// Finds out for certain whether the agent is signed in: opens a session, then asks it one tiny question,
/// because some agents only check their credentials when they are asked something. A permission asked
/// during the question is refused, and plan mode keeps it from changing anything.
async fn verify(conn: &std::sync::Arc<Conn>, harness: &Harness, cwd: &str, depth: Depth) -> AuthState {
    let session = match open_probe(conn, harness, cwd).await {
        Ok(session) => session,
        Err(state) => return state,
    };
    // An agent that says whether it is signed in answers the quick check without a question (which costs a
    // real prompt: on the Claude agent about 60,000 tokens of context).
    if depth == Depth::Quick
        && let Some(state) = conn.auth_state_soon().await
    {
        close_session(conn, &session).await;
        return if state == "authenticated" { AuthState::SignedIn } else { AuthState::NeedsSignIn(methods_in(&conn.caps(), conn.speaks_v2(), harness)) };
    }
    let (events, mut seen) = mpsc::unbounded_channel();
    let drain = tokio::spawn(async move {
        while let Some(event) = seen.recv().await {
            if let AcpEvent::Permission(request) = event {
                request.answer(false);
            }
        }
    });
    let cancel = CancellationToken::new();
    let limit = cancel.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(ANSWER_TIMEOUT).await;
        limit.cancel();
    });
    let prompt = [PromptPart::Text("Reply with the single word: ok".to_owned())];
    let request = TurnRequest { prompt: &prompt, earlier: &[], plan_mode: true, bypass: false, mcp_servers: &[], own: None };
    let thread = format!("{DELEGATE}auth-{}", harness.id);
    let end = turn::run(conn, harness, &session, &thread, &request, &events, &cancel).await;
    timer.abort();
    drop(events);
    let _ = drain.await;
    close_session(conn, &session).await;
    match end.outcome {
        Outcome::Finished => AuthState::SignedIn,
        Outcome::Failed(message) if says_sign_in(&message) => AuthState::NeedsSignIn(methods_in(&conn.caps(), conn.speaks_v2(), harness)),
        Outcome::Failed(message) => AuthState::Failed(message),
        Outcome::Stopped => AuthState::Failed(format!("{} didn't answer in {} seconds.", harness.name, ANSWER_TIMEOUT.as_secs())),
    }
}

/// The check is not a conversation: close its session where the agent can.
async fn close_session(conn: &Conn, session: &str) {
    if conn.cap("/capabilities/session", "/agentCapabilities/sessionCapabilities/close") {
        let _ = conn.request("session/close", json!({ "sessionId": session }), Some(Duration::from_secs(10))).await;
    }
}

/// How sure a check has to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// Believe what the agent says about being signed in; ask it a question only if it says nothing. Fast and
    /// free, but an agent can say "signed in" about a login that has since expired.
    Quick,
    /// Always ask the agent one tiny question. A few seconds and a real prompt.
    Thorough,
}

fn working_folder() -> String {
    std::env::current_dir().map_or_else(|_| ".".to_owned(), |d| d.display().to_string())
}

/// Whether the agent can be used now, asked of a fresh process with a real question (it costs one tiny
/// prompt).
pub async fn check(id: &str, depth: Depth) -> Result<AuthState, String> {
    let harness = find(id).await?;
    if !harness.available {
        return Err(format!("{} isn't installed.", harness.name));
    }
    if harness.needs_setup() {
        return Err(format!("{} isn't set up yet.", harness.name));
    }
    let cwd = working_folder();
    let conn = spawn(&harness, &cwd).await?;
    let state = verify(&conn, &harness, &cwd, depth).await;
    trace(&harness.name, format!("sign-in check: {state:?}"));
    Ok(state)
}

/// The ways an agent can be signed in, without asking it anything else. For an agent that failed a turn with
/// "sign in", where checking again would only fail the same way.
pub async fn options(id: &str) -> Result<Vec<AuthMethod>, String> {
    let harness = find(id).await?;
    if !harness.available || harness.needs_setup() {
        return Err(format!("{} isn't ready to sign in to.", harness.name));
    }
    let conn = spawn(&harness, &working_folder()).await?;
    Ok(methods_in(&conn.caps(), conn.speaks_v2(), &harness))
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
    Ok(verify(&conn, &harness, &cwd, Depth::Thorough).await)
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

    /// An agent that has finished `initialize` as a v2 agent offering one browser sign-in.
    async fn ready_agent() -> (std::sync::Arc<Conn>, super::super::conn::testing::Peer) {
        use super::super::conn::testing::peer;
        let (conn, mut agent) = peer("mock");
        let ready = tokio::spawn({
            let conn = conn.clone();
            async move { super::super::conn::handshake(&conn, &harness("claude-acp")).await }
        });
        agent.hello_version(2, json!({}), json!([{"methodId": "browser", "name": "Browser", "type": "agent"}])).await;
        assert!(ready.await.is_ok_and(|r| r.is_ok()));
        (conn, agent)
    }

    #[tokio::test]
    async fn an_agent_that_refuses_a_session_is_not_signed_in_and_lists_the_ways() {
        let (conn, mut agent) = ready_agent().await;
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { verify(&conn, &harness("claude-acp"), "/p", Depth::Thorough).await }
        });
        let request = agent.recv().await;
        assert_eq!(request["method"], "session/new");
        agent.refuse(&request["id"], -32000, "authentication required", Value::Null).await;
        let state = asked.await.unwrap_or(AuthState::Failed(String::new()));
        assert!(matches!(&state, AuthState::NeedsSignIn(m) if m.len() == 1 && m[0].id == "browser"), "{state:?}");
    }

    #[tokio::test]
    async fn an_agent_that_opens_a_session_but_fails_the_question_is_not_signed_in() {
        // The Claude adapter checks its credentials only when it is asked something.
        let (conn, mut agent) = ready_agent().await;
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { verify(&conn, &harness("claude-acp"), "/p", Depth::Thorough).await }
        });
        let open = agent.open("probe").await;
        assert_eq!(open["method"], "session/new");
        let prompt = agent.prompt().await;
        agent.refuse(&prompt["id"], -32000, "Authentication required: OAuth session expired", Value::Null).await;
        let state = asked.await.unwrap_or(AuthState::Failed(String::new()));
        assert!(matches!(&state, AuthState::NeedsSignIn(_)), "{state:?}");
    }

    #[tokio::test]
    async fn an_agent_that_answers_the_question_is_signed_in() {
        let (conn, mut agent) = ready_agent().await;
        let asked = tokio::spawn({
            let conn = conn.clone();
            async move { verify(&conn, &harness("claude-acp"), "/p", Depth::Thorough).await }
        });
        agent.open("probe").await;
        let prompt = agent.prompt().await;
        assert_eq!(prompt["params"]["prompt"][0]["text"], "Reply with the single word: ok");
        agent.update("probe", json!({"sessionUpdate": "agent_message_chunk", "content": {"text": "ok"}})).await;
        agent.update("probe", json!({"sessionUpdate": "state_update", "state": "idle"})).await;
        agent.answer(&prompt["id"], json!({})).await;
        assert_eq!(asked.await.unwrap_or(AuthState::Failed(String::new())), AuthState::SignedIn);
    }

    #[tokio::test]
    async fn a_quick_check_believes_what_the_agent_says_without_asking_it_a_question() {
        for (state, signed_in) in [("authenticated", true), ("unauthenticated", false)] {
            let (conn, mut agent) = ready_agent().await;
            let asked = tokio::spawn({
                let conn = conn.clone();
                async move { verify(&conn, &harness("claude-acp"), "/p", Depth::Quick).await }
            });
            agent.send(json!({"jsonrpc": "2.0", "method": "auth/status_update", "params": {"state": state}})).await;
            agent.open("probe").await;
            let result = asked.await.unwrap_or(AuthState::Failed(String::new()));
            if signed_in {
                assert_eq!(result, AuthState::SignedIn);
            } else {
                assert!(matches!(result, AuthState::NeedsSignIn(_)), "{result:?}");
            }
            assert!(agent.next_within(150).await.is_none_or(|frame| frame["method"] != "session/prompt"), "no question was asked");
        }
    }

    #[test]
    fn the_agents_own_sign_in_words_are_recognised() {
        assert!(says_sign_in("Claude Agent needs you to sign in. Run `claude` once"));
        assert!(says_sign_in("Authentication required: Failed to authenticate"));
        assert!(!says_sign_in("rate limit reached"));
    }
}
