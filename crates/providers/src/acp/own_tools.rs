//! Vivianne's own tools, offered to a coding agent as an MCP server.
//!
//! A coding agent that runs inside Vivianne (Claude Code, Codex...) has its own file and shell tools but not
//! Vivianne's: the user's memory, their other threads. This offers a short, fixed list of them over the Model
//! Context Protocol, in whichever of two ways the agent can reach it: MCP over ACP (`mcp/connect`, `mcp/message`,
//! answered on the agent's own connection, nothing listening) or a local HTTP endpoint behind a bearer token.
//!
//! What is offered is decided by the app (it hands in the list); what is run for a call is too (it hands in the
//! means). Here is only the protocol, and the rules that do not depend on which tools there are:
//! - A call to a tool that is not on the list is refused, however it is asked for.
//! - A call outside a running turn is refused: nobody is watching for its approval.
//! - The HTTP endpoint listens on loopback only, and wants the token and a loopback `Host`, so a page in the user's
//!   browser cannot reach it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The name the agent sees the server under, and its ACP `serverId`.
pub const SERVER_NAME: &str = "vivianne";

/// Largest request body the HTTP endpoint reads.
const MAX_BODY: usize = 8 * 1024 * 1024;
/// Calls running at once through one connection.
const MAX_AT_ONCE: usize = 4;

/// One tool offered to the agent.
#[derive(Debug, Clone, PartialEq)]
pub struct OfferedTool {
    pub name: String,
    /// What it is for, in a line, for the instructions the agent is given.
    pub summary: String,
    /// What the agent is told it does, in full.
    pub description: String,
    /// The shape of its arguments (JSON Schema).
    pub schema: Value,
}

/// What a call returned, as MCP content blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub content: Vec<Value>,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>, is_error: bool) -> Self {
        Self { content: vec![json!({ "type": "text", "text": text.into() })], is_error }
    }

    fn result(self) -> Value {
        json!({ "content": self.content, "isError": self.is_error })
    }
}

/// Runs one call for the turn that is on: its name and arguments in, its output out.
pub type CallFn = Arc<dyn Fn(String, Value) -> BoxFuture<'static, ToolOutput> + Send + Sync>;

/// What a turn offers the agent: the tools, and the means to run a call.
#[derive(Clone)]
pub struct OwnTools {
    pub tools: Vec<OfferedTool>,
    pub call: CallFn,
}

impl std::fmt::Debug for OwnTools {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnTools").field("tools", &self.tools.iter().map(|t| &t.name).collect::<Vec<_>>()).finish_non_exhaustive()
    }
}

/// A JSON-RPC error: its code and message.
pub type McpError = (i64, String);

/// The server, for one connection to one agent. The turn that is running hands it the means to run a call;
/// between turns there is none.
pub struct Bridge {
    offered: Mutex<Vec<OfferedTool>>,
    turn: Mutex<Option<CallFn>>,
    slots: tokio::sync::Semaphore,
}

impl Bridge {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self { offered: Mutex::new(Vec::new()), turn: Mutex::new(None), slots: tokio::sync::Semaphore::new(MAX_AT_ONCE) })
    }

    /// Sets what is offered. The agent lists the tools when its session opens, so this comes first.
    pub fn offer(&self, tools: Vec<OfferedTool>) {
        *self.offered.lock().unwrap_or_else(PoisonError::into_inner) = tools;
    }

    /// A turn starts: calls now run through `call`.
    pub fn begin(&self, call: CallFn) {
        *self.turn.lock().unwrap_or_else(PoisonError::into_inner) = Some(call);
    }

    /// The turn is over: a call that arrives now is refused.
    pub fn end(&self) {
        *self.turn.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn descriptors(&self) -> Vec<Value> {
        self.offered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "inputSchema": t.schema }))
            .collect()
    }

    fn is_offered(&self, name: &str) -> bool {
        self.offered.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|t| t.name == name)
    }

    /// Answers one MCP request. `Ok` is the result; `Err` a JSON-RPC error.
    pub async fn request(&self, method: &str, params: &Value) -> Result<Value, McpError> {
        match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-03-26"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.descriptors() })),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                if !self.is_offered(name) {
                    return Err((-32602, format!("unknown tool: {name}")));
                }
                let args = params.get("arguments").filter(|a| a.is_object()).cloned().unwrap_or_else(|| json!({}));
                let call = self.turn.lock().unwrap_or_else(PoisonError::into_inner).clone();
                let Some(call) = call else {
                    return Ok(ToolOutput::text(
                        "Vivianne has no turn running to answer this call in, so it was not run. Ask again while working on something for the user.",
                        true,
                    )
                    .result());
                };
                let _slot = self.slots.acquire().await.map_err(|_| (-32603, "the server is closing".to_owned()))?;
                Ok(call(name.to_owned(), args).await.result())
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        }
    }
}

// ── The user's own MCP servers ───────────────────────────────────────────

/// An MCP server the user has added, handed to the agent so it has the same connected tools Vivianne does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerSpec {
    Stdio { name: String, command: String, args: Vec<String>, env: Vec<(String, String)> },
    Http { name: String, url: String, headers: Vec<(String, String)> },
}

/// The servers in the shape `session/new` takes. Left out: a transport the agent did not say it takes. v2 makes
/// every transport name itself; in v1 stdio is the default and has no `type`.
#[must_use]
pub fn servers_json(specs: &[McpServerSpec], v2: bool, stdio: bool, http: bool) -> Vec<Value> {
    let pairs = |m: &[(String, String)]| m.iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>();
    specs
        .iter()
        .filter_map(|spec| match spec {
            McpServerSpec::Stdio { name, command, args, env } if stdio => {
                // The protocol asks for an absolute path.
                let path = which::which(command).map_or_else(|_| command.clone(), |p| p.display().to_string());
                let mut server = json!({ "name": name, "command": path, "args": args, "env": pairs(env) });
                if v2 {
                    server["type"] = json!("stdio");
                }
                Some(server)
            }
            McpServerSpec::Http { name, url, headers } if http => Some(json!({ "type": "http", "name": name, "url": url, "headers": pairs(headers) })),
            _ => None,
        })
        .collect()
}

// ── What the agent is told ───────────────────────────────────────────────

/// What to tell the agent about where it is running, or nothing when there is nothing to say: its tools, and
/// whether plan mode is on.
#[must_use]
pub fn instructions(plan_mode: bool, tools: &[OfferedTool]) -> Option<String> {
    if !plan_mode && tools.is_empty() {
        return None;
    }
    let mut s = String::from("<vivianne>\nYou are running inside Vivianne, a desktop app, working for the user in their project.\n");
    if !tools.is_empty() {
        s.push_str(&format!("Vivianne's own tools are offered to you as MCP tools from the \"{SERVER_NAME}\" server:\n"));
        for tool in tools {
            s.push_str(&format!("- {}: {}\n", tool.name, tool.summary));
        }
        s.push_str("Use recall at the start of a task that may relate to earlier work. Each call is checked by Vivianne.\n");
    }
    s.push_str(if plan_mode {
        "Plan mode is on: research and plan only. Do not change files or run anything that changes something; Vivianne refuses it.\n"
    } else {
        "Plan mode is off.\n"
    });
    s.push_str("</vivianne>");
    Some(s)
}

/// What to send now, given what was sent last and the state now: the text when it changed, a note when plan mode
/// went off and nothing else is left to say, else nothing. Returns what to send and what to remember as sent.
#[must_use]
pub fn next_instructions(last: &Option<String>, plan_mode: bool, tools: &[OfferedTool]) -> (Option<String>, Option<String>) {
    let want = instructions(plan_mode, tools);
    if want == *last {
        return (None, want);
    }
    match want {
        Some(text) => (Some(text.clone()), Some(text)),
        // Plan mode was on and there is nothing else to say: the agent still believes it.
        None => (Some("<vivianne>\nPlan mode is off: you may change files and run commands again.\n</vivianne>".to_owned()), None),
    }
}

// ── The local HTTP endpoint ──────────────────────────────────────────────

/// A listening endpoint for one agent: its URL and the bearer token it wants. Closing it is dropping it.
pub struct Http {
    pub url: String,
    pub token: String,
    task: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http").field("url", &self.url).finish_non_exhaustive()
    }
}

impl Drop for Http {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Listens on loopback, on a port the system picks, for MCP over HTTP. Every request needs the token and a
/// loopback `Host`.
pub async fn serve(bridge: Arc<Bridge>) -> std::io::Result<Http> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
    let wanted = token.clone();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let (bridge, token) = (bridge.clone(), wanted.clone());
            tokio::spawn(async move {
                // A call may wait on an approval, so the ceiling is generous.
                let _ = tokio::time::timeout(Duration::from_secs(1200), connection(sock, bridge, token, port)).await;
            });
        }
    });
    Ok(Http { url: format!("http://127.0.0.1:{port}/mcp"), token, task })
}

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn status_text(code: u16) -> &'static str {
    match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Not Implemented",
    }
}

/// Reads one request: head, then as many body bytes as it says it has.
async fn read_request(sock: &mut tokio::net::TcpStream) -> Result<Request, u16> {
    let mut buf: Vec<u8> = Vec::new();
    let head_end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buf.len() > 16 * 1024 {
            return Err(431);
        }
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(30), sock.read(&mut chunk)).await.map_err(|_| 400u16)?.map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| 400u16)?.to_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split_whitespace();
    let (method, path) = (first.next().unwrap_or("").to_owned(), first.next().unwrap_or("").to_owned());
    let headers: HashMap<String, String> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    if headers.contains_key("transfer-encoding") {
        return Err(501);
    }
    let length: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if length > MAX_BODY {
        return Err(413);
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(30), sock.read(&mut chunk)).await.map_err(|_| 400u16)?.map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok(Request { method, path, headers, body })
}

/// Whether two secrets are equal, in the same time whatever they share.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The status and the JSON body (if any) for a request.
async fn answer(req: &Request, bridge: &Bridge, token: &str, port: u16) -> (u16, Option<Value>) {
    let loopback = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    if !req.headers.get("host").is_some_and(|h| loopback.iter().any(|l| l == h)) {
        return (403, None);
    }
    if req.path.split('?').next() != Some("/mcp") {
        return (404, None);
    }
    let bearer = req.headers.get("authorization").and_then(|a| a.strip_prefix("Bearer ")).unwrap_or("");
    if !same(bearer.as_bytes(), token.as_bytes()) {
        return (401, None);
    }
    if req.method != "POST" {
        return (405, None);
    }
    let Ok(msg) = serde_json::from_slice::<Value>(&req.body) else {
        return (400, Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "parse error" } })));
    };
    if !msg.is_object() {
        return (400, Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32600, "message": "one message per request" } })));
    }
    // No id: a notification, which is not answered.
    let Some(id) = msg.get("id").cloned() else { return (202, None) };
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let reply = match bridge.request(method, msg.get("params").unwrap_or(&Value::Null)).await {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    };
    (200, Some(reply))
}

async fn connection(mut sock: tokio::net::TcpStream, bridge: Arc<Bridge>, token: String, port: u16) {
    let (status, body) = match read_request(&mut sock).await {
        Ok(req) => answer(&req, &bridge, &token, port).await,
        Err(code) => (code, None),
    };
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut head = format!("HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n", status_text(status), body.len());
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    match status {
        401 => head.push_str("WWW-Authenticate: Bearer\r\n"),
        405 => head.push_str("Allow: POST\r\n"),
        _ => {}
    }
    head.push_str("\r\n");
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body.as_bytes()).await;
    let _ = sock.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> OfferedTool {
        OfferedTool { name: name.into(), summary: format!("does {name}"), description: format!("Does {name}."), schema: json!({ "type": "object", "properties": {} }) }
    }

    fn bridge() -> Arc<Bridge> {
        let bridge = Bridge::new();
        bridge.offer(vec![tool("recall"), tool("remember")]);
        bridge
    }

    fn echo() -> CallFn {
        Arc::new(|name, args| Box::pin(async move { ToolOutput::text(format!("{name}:{args}"), false) }))
    }

    #[tokio::test]
    async fn it_introduces_itself_and_lists_only_what_is_offered() {
        let bridge = bridge();
        let hello = bridge.request("initialize", &json!({ "protocolVersion": "2025-06-18" })).await.expect("answers");
        assert_eq!(hello["protocolVersion"], "2025-06-18", "the version the client asked for is the one answered");
        assert_eq!(hello["serverInfo"]["name"], "vivianne");
        assert_eq!(bridge.request("ping", &Value::Null).await.expect("answers"), json!({}));
        let listed = bridge.request("tools/list", &Value::Null).await.expect("answers");
        let names: Vec<&str> = listed["tools"].as_array().expect("a list").iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(names, ["recall", "remember"]);
        assert_eq!(listed["tools"][0]["inputSchema"]["type"], "object");
        assert_eq!(bridge.request("resources/list", &Value::Null).await.expect_err("not a method").0, -32601);
    }

    #[tokio::test]
    async fn a_call_runs_only_for_an_offered_tool_and_only_while_a_turn_is_running() {
        let bridge = bridge();
        let call = |name: &str| json!({ "name": name, "arguments": { "q": 1 } });
        // Nobody is watching: refused, as a tool error the agent can read.
        let idle = bridge.request("tools/call", &call("recall")).await.expect("answers");
        assert_eq!(idle["isError"], true);
        assert!(idle["content"][0]["text"].as_str().is_some_and(|t| t.contains("no turn running")));

        bridge.begin(echo());
        let ran = bridge.request("tools/call", &call("recall")).await.expect("answers");
        assert_eq!((ran["isError"].clone(), ran["content"][0]["text"].clone()), (json!(false), json!("recall:{\"q\":1}")));
        // A tool that is not on the list is not run, however it is asked for.
        for name in ["vault", "shell", "write", "nope"] {
            assert_eq!(bridge.request("tools/call", &call(name)).await.expect_err("refused").0, -32602, "{name}");
        }
        bridge.end();
        assert_eq!(bridge.request("tools/call", &call("recall")).await.expect("answers")["isError"], true, "the turn is over");
    }

    #[test]
    fn the_agent_is_told_what_it_has_and_only_when_that_changed() {
        assert_eq!(instructions(false, &[]), None, "nothing to say");
        let tools = [tool("recall")];
        let text = instructions(false, &tools).expect("something to say");
        assert!(text.contains("recall: does recall") && text.contains("Plan mode is off"));
        let plan = instructions(true, &[]).expect("something to say");
        assert!(plan.contains("Plan mode is on") && !plan.contains("recall"));

        // The first turn says it; the same state again says nothing.
        let (send, last) = next_instructions(&None, true, &[]);
        assert!(send.is_some());
        assert_eq!(next_instructions(&last, true, &[]).0, None);
        // Plan mode going off with nothing else to say still tells it.
        let (send, last) = next_instructions(&last, false, &[]);
        assert!(send.is_some_and(|s| s.contains("Plan mode is off")));
        assert_eq!(last, None);
        assert_eq!(next_instructions(&last, false, &[]).0, None, "and then it is quiet");
        // With tools, the text carries the plan line itself and is sent whole when it changes.
        let (_, last) = next_instructions(&None, false, &tools);
        let (send, _) = next_instructions(&last, true, &tools);
        assert!(send.is_some_and(|s| s.contains("Plan mode is on")));
    }

    /// One raw HTTP exchange with the endpoint.
    async fn exchange(port: u16, request: &str) -> String {
        let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.expect("connects");
        sock.write_all(request.as_bytes()).await.expect("writes");
        let mut out = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), sock.read_to_end(&mut out)).await.expect("an answer").expect("reads");
        String::from_utf8_lossy(&out).into_owned()
    }

    fn post(token: Option<&str>, host: &str, body: &str) -> String {
        let auth = token.map(|t| format!("Authorization: Bearer {t}\r\n")).unwrap_or_default();
        format!("POST /mcp HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    #[tokio::test]
    async fn the_http_endpoint_wants_the_token_and_a_loopback_host_and_answers_json_rpc() {
        let bridge = bridge();
        bridge.begin(echo());
        let http = serve(bridge).await.expect("a port");
        let port: u16 = http.url.rsplit(':').next().expect("a port").trim_end_matches("/mcp").parse().expect("a number");
        let host = format!("127.0.0.1:{port}");
        let list = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;

        assert!(exchange(port, &post(None, &host, list)).await.starts_with("HTTP/1.1 401"));
        assert!(exchange(port, &post(Some("not-it"), &host, list)).await.starts_with("HTTP/1.1 401"));
        let rebinding = exchange(port, &post(Some(&http.token), "evil.example:80", list)).await;
        assert!(rebinding.starts_with("HTTP/1.1 403"), "a page in the user's browser can't reach it by name: {rebinding}");

        let ok = exchange(port, &post(Some(&http.token), &host, list)).await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        let body: Value = serde_json::from_str(ok.split("\r\n\r\n").nth(1).unwrap_or_default()).expect("json");
        assert_eq!((body["jsonrpc"].clone(), body["id"].clone()), (json!("2.0"), json!(1)));
        assert!(body["result"]["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["name"] == "recall")));

        let called = exchange(port, &post(Some(&http.token), &host, r#"{"jsonrpc":"2.0","id":"c","method":"tools/call","params":{"name":"recall","arguments":{"q":"x"}}}"#)).await;
        assert!(called.contains("recall:{\\\"q\\\":\\\"x\\\"}"), "{called}");
        let refused = exchange(port, &post(Some(&http.token), &host, r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"vault","arguments":{}}}"#)).await;
        assert!(refused.contains("-32602"), "{refused}");

        let note = exchange(port, &post(Some(&http.token), &host, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)).await;
        assert!(note.starts_with("HTTP/1.1 202"), "a notification is accepted, not answered: {note}");
        assert!(exchange(port, &post(Some(&http.token), &host, "[]")).await.starts_with("HTTP/1.1 400"));
        let junk = exchange(port, &post(Some(&http.token), &host, "not json")).await;
        assert!(junk.starts_with("HTTP/1.1 400") && junk.contains("-32700"), "{junk}");
        let get = exchange(port, &format!("GET /mcp HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\n\r\n", http.token)).await;
        assert!(get.starts_with("HTTP/1.1 405"), "{get}");
        let elsewhere = exchange(port, &format!("POST /other HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Length: 0\r\n\r\n", http.token)).await;
        assert!(elsewhere.starts_with("HTTP/1.1 404"), "{elsewhere}");
        let chunked = exchange(port, &format!("POST /mcp HTTP/1.1\r\nHost: {host}\r\nTransfer-Encoding: chunked\r\n\r\n")).await;
        assert!(chunked.starts_with("HTTP/1.1 501"), "{chunked}");

        // Closing it closes the port.
        drop(http);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err(), "nothing listens once it is dropped");
    }

    #[test]
    fn the_users_servers_go_to_the_agent_in_the_shape_its_version_takes_and_only_over_what_it_takes() {
        let specs = [
            McpServerSpec::Stdio { name: "mem".into(), command: "no-such-program-here".into(), args: vec!["-y".into()], env: vec![("K".into(), "V".into())] },
            McpServerSpec::Http { name: "web".into(), url: "https://x.test/mcp".into(), headers: vec![("A".into(), "B".into())] },
        ];
        let v1 = servers_json(&specs, false, true, true);
        assert_eq!(v1.len(), 2);
        assert!(v1[0].get("type").is_none() && v1[0]["env"][0]["name"] == "K", "{}", v1[0]);
        assert_eq!(v1[1]["type"], "http");
        let v2 = servers_json(&specs, true, true, false);
        assert_eq!((v2.len(), v2[0]["type"].clone()), (1, json!("stdio")), "no http: only the program");
        assert!(servers_json(&specs, true, false, false).is_empty());
    }

    /// An agent that reaches MCP over its own connection (`mcp/connect`, `mcp/message`) is answered there, with the
    /// same rules as over HTTP.
    #[tokio::test]
    async fn an_agent_can_reach_the_tools_over_its_own_connection() {
        let (conn, mut agent) = super::super::conn::testing::peer("a");
        conn.mcp.offer(vec![tool("recall")]);
        conn.mcp.begin(echo());
        let ask = |id: u64, method: &str, params: Value| {
            let conn = conn.clone();
            let method = method.to_owned();
            tokio::spawn(async move { conn.answer_mcp(json!(id), method, params).await });
        };
        ask(1, "mcp/connect", json!({ "serverId": "vivianne" }));
        let connected = agent.recv().await;
        assert!(connected["result"]["connectionId"].as_str().is_some_and(|c| c.starts_with("vivianne-")), "{connected}");
        ask(2, "mcp/connect", json!({ "serverId": "someone-else" }));
        assert_eq!(agent.recv().await["error"]["code"], -32602);
        ask(3, "mcp/message", json!({ "method": "tools/list" }));
        assert_eq!(agent.recv().await["result"]["tools"][0]["name"], "recall");
        ask(4, "mcp/message", json!({ "method": "tools/call", "params": { "name": "recall", "arguments": { "q": 1 } } }));
        assert_eq!(agent.recv().await["result"]["content"][0]["text"], "recall:{\"q\":1}");
        ask(5, "mcp/message", json!({ "method": "tools/call", "params": { "name": "shell", "arguments": {} } }));
        assert_eq!(agent.recv().await["error"]["code"], -32602, "a tool that is not offered is not run");
    }

    #[test]
    fn a_secret_is_compared_whole() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd") && !same(b"abc", b"ab") && !same(b"", b"a"));
    }
}
