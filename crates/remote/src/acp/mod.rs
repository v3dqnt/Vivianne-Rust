//! `viv acp`: other programs drive Vivianne's agent over the Agent Client Protocol (version 1), one JSON-RPC message
//! per line. The client side of the protocol, where Vivianne drives other agents, is `vivianne_providers::acp`.
//!
//! Port of `@deepseek-ai/dsh-acp`. Gaps: `docs/acp-gaps.md`.

mod host;
mod sessions;
mod updates;

#[cfg(test)]
mod tests;

pub use host::{Built, Host, Standard, Threads, standard_build};

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};
use tokio::sync::{mpsc, oneshot};

use sessions::Sessions;

const PROTOCOL_VERSION: u64 = 1;
const PARSE_ERROR: i64 = -32700;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A refused request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Refused {
    code: i64,
    message: String,
}

impl Refused {
    fn params(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_PARAMS,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: INTERNAL_ERROR,
            message: message.into(),
        }
    }
}

/// The way out to the client: messages, and requests of our own that wait for an answer.
#[derive(Clone)]
struct Client {
    out: mpsc::UnboundedSender<Value>,
    next: Arc<AtomicU64>,
    waiting: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
}

impl Client {
    fn send(&self, message: Value) {
        // A client that is gone gets nothing; the read loop notices and ends the sessions.
        let _ = self.out.send(message);
    }

    fn answer(&self, id: &Value, result: Result<Value, Refused>) {
        self.send(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
        });
    }

    fn update(&self, session: &str, update: Value) {
        self.send(json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": update },
        }));
    }

    /// Sends a request; the response, or nothing if the client goes first.
    fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        lock(&self.waiting).insert(id, tx);
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx
    }

    fn response(&self, message: Value) {
        let waiting = message["id"]
            .as_u64()
            .and_then(|id| lock(&self.waiting).remove(&id));
        if let Some(waiting) = waiting {
            let _ = waiting.send(message);
        }
    }

    /// Nobody will answer now: every pending question is a no.
    fn abandon(&self) {
        lock(&self.waiting).clear();
    }
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "agentInfo": { "name": "vivianne", "version": env!("CARGO_PKG_VERSION") },
        "agentCapabilities": {
            "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false },
            "sessionCapabilities": { "close": {}, "list": {}, "resume": {} },
        },
        "authMethods": [],
    })
}

/// Serves one client until it closes its end, then ends every session and waits for their turns.
pub async fn serve<R, W>(host: Host, mut input: R, mut output: W) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, mut queued) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(message) = queued.recv().await {
            let mut line = message.to_string();
            line.push('\n');
            if output.write_all(line.as_bytes()).await.is_err() || output.flush().await.is_err() {
                break;
            }
        }
    });
    let client = Client {
        out,
        next: Arc::new(AtomicU64::new(1)),
        waiting: Arc::default(),
    };
    let sessions = Sessions::new(host, client.clone());
    let mut line = String::new();
    loop {
        line.clear();
        if input.read_line(&mut line).await? == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            client.answer(
                &Value::Null,
                Err(Refused {
                    code: PARSE_ERROR,
                    message: "not a JSON message".to_owned(),
                }),
            );
            continue;
        };
        let Some(method) = message["method"].as_str() else {
            client.response(message);
            continue;
        };
        let params = &message["params"];
        let Some(id) = message.get("id") else {
            // Notifications; one nothing here knows is ignored, as the protocol asks.
            if method == "session/cancel" {
                sessions.cancel(params);
            }
            continue;
        };
        let result = match method {
            "initialize" => Ok(initialize()),
            "authenticate" => Ok(json!({})),
            "session/new" => sessions.create(params),
            "session/list" => sessions.list(params),
            "session/resume" => sessions.resume(params),
            "session/close" => sessions.close(params).await,
            // Answers when the turn ends, while this loop goes on reading.
            "session/prompt" => match sessions.prompt(id.clone(), params) {
                Ok(()) => continue,
                Err(refused) => Err(refused),
            },
            other => Err(Refused {
                code: METHOD_NOT_FOUND,
                message: format!("method not found: {other}"),
            }),
        };
        client.answer(id, result);
    }
    client.abandon();
    sessions.shut_down().await;
    drop(sessions);
    drop(client);
    let _ = writer.await;
    Ok(())
}
