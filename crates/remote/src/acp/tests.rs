//! A client talking to the server over an in-memory pipe, with a scripted model and conversations kept in memory.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader, DuplexStream};
use vivianne_core::ToolCall;
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};
use vivianne_runtime::Agent;
use vivianne_runtime::store::SavedThread;
use vivianne_tools::Toolbox;

use super::*;

type Replies = Arc<Mutex<VecDeque<Vec<ChatEvent>>>>;

struct Scripted(Replies);

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &'static str {
        "scripted"
    }

    async fn stream(&self, _request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let events = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(vec![ChatEvent::Done]);
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

#[derive(Default)]
struct Memory(Mutex<HashMap<String, SavedThread>>);

impl Threads for Memory {
    fn save(&self, thread: &SavedThread) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .insert(thread.id.clone(), thread.clone());
        Ok(())
    }

    fn load(&self, id: &str) -> Option<SavedThread> {
        self.0.lock().unwrap().get(id).cloned()
    }

    fn list(&self, folder: Option<&str>) -> Vec<(String, String)> {
        self.0
            .lock()
            .unwrap()
            .values()
            .filter(|t| folder.is_none_or(|f| f == t.workspace))
            .map(|t| (t.workspace.clone(), t.id.clone()))
            .collect()
    }

    fn lease(&self, _id: &str) -> Result<Box<dyn Send>, String> {
        Ok(Box::new(()))
    }
}

struct Peer {
    to: DuplexStream,
    from: BufReader<DuplexStream>,
    next: u64,
}

impl Peer {
    async fn send(&mut self, message: Value) {
        let line = format!("{message}\n");
        self.to.write_all(line.as_bytes()).await.unwrap();
    }

    async fn recv(&mut self) -> Value {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(10), self.from.read_line(&mut line));
        assert!(
            read.await.expect("answered in time").unwrap() > 0,
            "the server closed"
        );
        serde_json::from_str(&line).unwrap()
    }

    async fn call(&mut self, method: &str, params: Value) -> u64 {
        self.next += 1;
        let id = self.next;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        id
    }

    /// Reads until the response to `id`, returning it and everything that came before.
    async fn until(&mut self, id: u64) -> (Value, Vec<Value>) {
        let mut before = Vec::new();
        loop {
            let message = self.recv().await;
            if message["id"] == id && message.get("method").is_none() {
                return (message, before);
            }
            before.push(message);
        }
    }

    async fn ask(&mut self, method: &str, params: Value) -> Value {
        let id = self.call(method, params).await;
        self.until(id).await.0
    }
}

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viv-acp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn start(
    replies: Vec<Vec<ChatEvent>>,
) -> (
    Peer,
    Arc<Memory>,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let replies: Replies = Arc::new(Mutex::new(replies.into()));
    let memory = Arc::new(Memory::default());
    let host = Host {
        build: Arc::new(move |dir: &Path| {
            Ok(Built {
                agent: Agent::new(
                    Arc::new(Scripted(Arc::clone(&replies))),
                    "m",
                    None,
                    Toolbox::coding(dir),
                ),
                provider: "scripted".to_owned(),
                model: "m".to_owned(),
            })
        }),
        threads: Arc::clone(&memory) as Arc<dyn Threads>,
    };
    let (to, server_in) = tokio::io::duplex(1 << 16);
    let (server_out, from) = tokio::io::duplex(1 << 16);
    let server = tokio::spawn(serve(host, BufReader::new(server_in), server_out));
    let peer = Peer {
        to,
        from: BufReader::new(from),
        next: 0,
    };
    (peer, memory, server)
}

fn writes_a_file() -> Vec<Vec<ChatEvent>> {
    let call = ToolCall {
        id: "w1".to_owned(),
        name: "write".to_owned(),
        args: json!({ "path": "made.txt", "content": "hello" }),
    };
    vec![
        vec![ChatEvent::ToolCall(call), ChatEvent::Done],
        vec![ChatEvent::Text("finished".to_owned()), ChatEvent::Done],
    ]
}

async fn new_session(peer: &mut Peer, dir: &Path) -> String {
    let made = peer
        .ask("session/new", json!({ "cwd": dir, "mcpServers": [] }))
        .await;
    made["result"]["sessionId"]
        .as_str()
        .expect("a session")
        .to_owned()
}

fn words(text: &str) -> Value {
    json!([{ "type": "text", "text": text }])
}

/// Runs one prompt that wants to write, answering its question with `option`.
async fn prompt_answering(peer: &mut Peer, session: &str, option: &str) -> (Value, Vec<Value>) {
    let id = peer
        .call(
            "session/prompt",
            json!({ "sessionId": session, "prompt": words("write it") }),
        )
        .await;
    let mut seen = Vec::new();
    loop {
        let message = peer.recv().await;
        if message["method"] == "session/request_permission" {
            assert_eq!(message["params"]["toolCall"]["toolCallId"], "w1");
            assert!(
                message["params"]["toolCall"]["title"]
                    .as_str()
                    .unwrap()
                    .contains("made.txt")
            );
            let outcome = json!({ "outcome": { "outcome": "selected", "optionId": option } });
            peer.send(json!({ "jsonrpc": "2.0", "id": message["id"], "result": outcome }))
                .await;
        } else if message["id"] == id && message.get("method").is_none() {
            return (message, seen);
        }
        seen.push(message);
    }
}

fn updates_of<'a>(seen: &'a [Value], kind: &str) -> Vec<&'a Value> {
    seen.iter()
        .map(|m| &m["params"]["update"])
        .filter(|u| u["sessionUpdate"] == kind)
        .collect()
}

#[tokio::test]
async fn initialize_says_version_one_and_what_is_offered() {
    let (mut peer, _, server) = start(Vec::new());
    let hello = peer
        .ask("initialize", json!({ "protocolVersion": 2 }))
        .await;
    assert_eq!(hello["result"]["protocolVersion"], 1);
    assert_eq!(
        hello["result"]["agentCapabilities"]["promptCapabilities"]["image"],
        false
    );
    assert!(hello["result"]["agentCapabilities"]["sessionCapabilities"]["resume"].is_object());
    let unknown = peer.ask("session/load", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32601);
    peer.send(json!("not an object")).await;
    peer.to.write_all(b"{broken\n").await.unwrap();
    assert_eq!(peer.recv().await["error"]["code"], -32700);
    drop(peer);
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn an_allowed_write_happens_and_is_reported_and_saved() {
    let dir = folder("allow");
    let (mut peer, memory, _server) = start(writes_a_file());
    let session = new_session(&mut peer, &dir).await;
    let (response, seen) = prompt_answering(&mut peer, &session, "allow_once").await;
    assert_eq!(response["result"]["stopReason"], "end_turn", "{response}");
    assert_eq!(
        std::fs::read_to_string(dir.join("made.txt")).unwrap(),
        "hello"
    );
    assert_eq!(updates_of(&seen, "tool_call")[0]["kind"], "edit");
    assert_eq!(
        updates_of(&seen, "tool_call_update")[0]["status"],
        "completed"
    );
    assert_eq!(
        updates_of(&seen, "agent_message_chunk")[0]["content"]["text"],
        "finished"
    );
    let saved = memory.load(&session).expect("saved");
    assert_eq!(saved.title, "Write it");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_rejected_write_does_not_happen() {
    let dir = folder("reject");
    let (mut peer, _, _server) = start(writes_a_file());
    let session = new_session(&mut peer, &dir).await;
    let (response, seen) = prompt_answering(&mut peer, &session, "reject_once").await;
    assert_eq!(response["result"]["stopReason"], "end_turn");
    assert!(!dir.join("made.txt").exists());
    assert_eq!(updates_of(&seen, "tool_call_update")[0]["status"], "failed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cancel_ends_a_turn_waiting_for_permission_and_a_second_prompt_is_refused_meanwhile() {
    let dir = folder("cancel");
    let (mut peer, _, _server) = start(writes_a_file());
    let session = new_session(&mut peer, &dir).await;
    let first = peer
        .call(
            "session/prompt",
            json!({ "sessionId": session, "prompt": words("write it") }),
        )
        .await;
    while peer.recv().await["method"] != "session/request_permission" {}
    let second = peer
        .ask(
            "session/prompt",
            json!({ "sessionId": session, "prompt": words("and this") }),
        )
        .await;
    assert!(
        second["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already in flight"),
        "{second}"
    );
    peer.send(
        json!({ "jsonrpc": "2.0", "method": "session/cancel", "params": { "sessionId": session } }),
    )
    .await;
    let (response, _) = peer.until(first).await;
    assert_eq!(response["result"]["stopReason"], "cancelled", "{response}");
    assert!(!dir.join("made.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_client_that_leaves_while_asked_allows_nothing() {
    let dir = folder("leave");
    let (mut peer, _, server) = start(writes_a_file());
    let session = new_session(&mut peer, &dir).await;
    peer.call(
        "session/prompt",
        json!({ "sessionId": session, "prompt": words("write it") }),
    )
    .await;
    while peer.recv().await["method"] != "session/request_permission" {}
    drop(peer);
    tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("ends")
        .unwrap()
        .unwrap();
    assert!(!dir.join("made.txt").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_closed_session_is_listed_and_resumed_only_in_its_own_folder() {
    let dir = folder("resume");
    let other = folder("resume-other");
    let (mut peer, _, _server) = start(vec![vec![
        ChatEvent::Text("hi".to_owned()),
        ChatEvent::Done,
    ]]);
    let session = new_session(&mut peer, &dir).await;
    peer.ask(
        "session/prompt",
        json!({ "sessionId": session, "prompt": words("hello") }),
    )
    .await;
    assert_eq!(
        peer.ask("session/list", json!({})).await["result"]["sessions"],
        json!([]),
        "open ones are not listed"
    );
    let again = peer
        .ask(
            "session/resume",
            json!({ "sessionId": session, "cwd": dir }),
        )
        .await;
    assert!(
        again["error"]["message"]
            .as_str()
            .unwrap()
            .contains("already active")
    );
    assert!(
        peer.ask("session/close", json!({ "sessionId": session }))
            .await["result"]
            .is_object()
    );
    let listed = peer.ask("session/list", json!({})).await;
    assert_eq!(
        listed["result"]["sessions"][0]["sessionId"],
        session.as_str()
    );
    let wrong = peer
        .ask(
            "session/resume",
            json!({ "sessionId": session, "cwd": other }),
        )
        .await;
    assert!(
        wrong["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not match"),
        "{wrong}"
    );
    let resumed = peer
        .ask(
            "session/resume",
            json!({ "sessionId": session, "cwd": dir }),
        )
        .await;
    assert!(resumed["result"].is_object(), "{resumed}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&other);
}

#[tokio::test]
async fn what_a_session_cannot_be_made_from_is_refused() {
    let dir = folder("refuse");
    let (mut peer, _, _server) = start(Vec::new());
    for params in [
        json!({ "cwd": "relative", "mcpServers": [] }),
        json!({ "cwd": dir.join("nowhere"), "mcpServers": [] }),
        json!({}),
    ] {
        assert_eq!(
            peer.ask("session/new", params.clone()).await["error"]["code"],
            -32602,
            "{params}"
        );
    }
    let unknown = peer
        .ask(
            "session/prompt",
            json!({ "sessionId": "none", "prompt": words("x") }),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_brought_mcp_list_does_not_stop_a_session() {
    let dir = folder("mcp");
    let (mut peer, _, _server) = start(Vec::new());
    let made = peer
        .ask(
            "session/new",
            json!({ "cwd": dir, "mcpServers": [{ "name": "x", "command": "y" }] }),
        )
        .await;
    assert!(made["result"]["sessionId"].is_string(), "{made}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_prompt_keeps_the_persons_title_and_what_was_added_elsewhere() {
    let dir = folder("keep");
    let (mut peer, memory, _server) = start(Vec::new());
    let session = new_session(&mut peer, &dir).await;
    peer.ask(
        "session/prompt",
        json!({ "sessionId": session, "prompt": words("first") }),
    )
    .await;
    // The app renames the thread and adds to it while the session is idle.
    let mut saved = memory.load(&session).expect("saved");
    saved.title = "Mine".to_owned();
    saved.title_source = "user".to_owned();
    saved.mode = vivianne_core::Mode::Plan;
    saved.history.push(vivianne_core::Message::new(
        vivianne_core::Role::User,
        "added in the app",
    ));
    memory.save(&saved).unwrap();
    peer.ask(
        "session/prompt",
        json!({ "sessionId": session, "prompt": words("second") }),
    )
    .await;
    let after = memory.load(&session).expect("saved");
    assert_eq!(
        (after.title.as_str(), after.title_source.as_str()),
        ("Mine", "user")
    );
    assert_eq!(after.mode, vivianne_core::Mode::Plan);
    let said: Vec<&str> = after.history.iter().map(|m| m.text.as_str()).collect();
    assert!(
        said.contains(&"added in the app") && said.contains(&"second"),
        "{said:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Vivianne's own ACP client against its own server, answering every question with `allow`.
async fn client_writes(allow: bool, name: &str) -> (vivianne_providers::acp::TurnEnd, PathBuf, Vec<String>) {
    use vivianne_providers::acp::{AcpEvent, DELEGATED, PromptPart, TurnRequest, run_over};

    let dir = folder(name);
    let (peer, _, _server) = start(writes_a_file());
    let Peer { to, from, .. } = peer;
    let (events, mut seen) = tokio::sync::mpsc::unbounded_channel();
    let answer = tokio::spawn(async move {
        let mut tools = Vec::new();
        while let Some(event) = seen.recv().await {
            match event {
                AcpEvent::Permission(request) => request.answer(allow),
                AcpEvent::ToolStart { name, .. } => tools.push(name),
                _ => {}
            }
        }
        tools
    });
    let prompt = [PromptPart::Text("write it".to_owned())];
    let request = TurnRequest {
        prompt: &prompt,
        earlier: &[],
        plan_mode: false,
        bypass: false,
        mcp_servers: &[],
        own: None,
    };
    let (thread, cwd) = (format!("{DELEGATED}{name}"), dir.display().to_string());
    let cancel = tokio_util::sync::CancellationToken::new();
    let turn = run_over(
        Box::new(from),
        Box::new(to),
        &thread,
        &cwd,
        &request,
        &events,
        &cancel,
    );
    let end = tokio::time::timeout(Duration::from_secs(20), turn)
        .await
        .expect("in time")
        .expect("ran");
    drop(events);
    (end, dir, answer.await.unwrap())
}

#[tokio::test]
async fn vivianne_as_client_drives_vivianne_as_server() {
    use vivianne_providers::acp::Outcome;

    let (end, dir, tools) = client_writes(true, "own-allow").await;
    assert_eq!(end.outcome, Outcome::Finished, "{}", end.reply);
    assert_eq!(end.reply.trim(), "finished");
    assert_eq!(std::fs::read_to_string(dir.join("made.txt")).unwrap(), "hello");
    assert_eq!(tools.len(), 1, "{tools:?}");
    let _ = std::fs::remove_dir_all(&dir);

    let (_, dir, _) = client_writes(false, "own-reject").await;
    assert!(!dir.join("made.txt").exists(), "a no from our client is a no to our server");
    let _ = std::fs::remove_dir_all(&dir);
}
