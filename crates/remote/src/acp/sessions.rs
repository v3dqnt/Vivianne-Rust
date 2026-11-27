//! The sessions of one connection: create, list, resume, prompt, cancel, close.
//!
//! Port of `@deepseek-ai/dsh-acp/index.ts` and `session.ts`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Mode, Role, Thinking};
use vivianne_runtime::store::SavedThread;
use vivianne_runtime::{Agent, AgentEvent};

use super::host::{Host, record};
use super::{Client, Refused, lock, updates};

struct Session {
    agent: Arc<Agent>,
    provider: String,
    model: String,
    folder: String,
    /// `None` while a prompt runs.
    history: Option<Vec<Message>>,
    turn: Option<(CancellationToken, JoinHandle<()>)>,
}

type Table = Arc<Mutex<HashMap<String, Session>>>;

pub(super) struct Sessions {
    host: Host,
    client: Client,
    table: Table,
}

fn session_id(params: &Value) -> Result<&str, Refused> {
    params["sessionId"]
        .as_str()
        .ok_or_else(|| Refused::params("`sessionId` must be a string"))
}

/// The folder a session works in: an absolute path to one that exists.
fn folder(params: &Value) -> Result<PathBuf, Refused> {
    let cwd = params["cwd"]
        .as_str()
        .ok_or_else(|| Refused::params("`cwd` must be a string"))?;
    let path = Path::new(cwd);
    if !path.is_absolute() {
        return Err(Refused::params(format!(
            "cwd must be an absolute path: {cwd}"
        )));
    }
    if !path.is_dir() {
        return Err(Refused::params(format!("cwd is not a folder: {cwd}")));
    }
    // Servers of a session's own are not mounted. A client sends the person's list as a matter of course (Vivianne's
    // own does), so the session goes ahead and says what it goes without.
    if let Some(servers) = params["mcpServers"].as_array().filter(|s| !s.is_empty()) {
        let names: Vec<&str> = servers.iter().filter_map(|s| s["name"].as_str()).collect();
        eprintln!(
            "viv acp: MCP servers a session brings are not mounted: {}",
            names.join(", ")
        );
    }
    Ok(path.to_path_buf())
}

/// Whether two spellings name one folder.
fn same_folder(a: &str, b: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    real(Path::new(a)) == real(b)
}

impl Sessions {
    pub(super) fn new(host: Host, client: Client) -> Self {
        Self {
            host,
            client,
            table: Table::default(),
        }
    }

    fn open(&self, id: &str, folder: &Path, history: Vec<Message>) -> Result<(), Refused> {
        let built = (self.host.build)(folder).map_err(Refused::internal)?;
        // The client decides what may happen: everything that needs a yes is asked of it.
        built.agent.set_mode(Mode::Ask);
        built.agent.set_thread(id);
        lock(&self.table).insert(
            id.to_owned(),
            Session {
                agent: Arc::new(built.agent),
                provider: built.provider,
                model: built.model,
                folder: folder.display().to_string(),
                history: Some(history),
                turn: None,
            },
        );
        Ok(())
    }

    pub(super) fn create(&self, params: &Value) -> Result<Value, Refused> {
        let folder = folder(params)?;
        let id = uuid::Uuid::new_v4().to_string();
        self.open(&id, &folder, Vec::new())?;
        Ok(json!({ "sessionId": id }))
    }

    pub(super) fn resume(&self, params: &Value) -> Result<Value, Refused> {
        let id = session_id(params)?;
        let folder = folder(params)?;
        if lock(&self.table).contains_key(id) {
            return Err(Refused::params(format!("session is already active: {id}")));
        }
        let saved = self
            .host
            .threads
            .load(id)
            .ok_or_else(|| Refused::params(format!("session is not resumable: {id}")))?;
        if !same_folder(&saved.workspace, &folder) {
            return Err(Refused::params(format!(
                "session cwd does not match: {}",
                folder.display()
            )));
        }
        self.open(id, &folder, saved.history)?;
        Ok(json!({}))
    }

    /// Saved sessions that are not open here, newest first. One page: the store already caps what it keeps.
    pub(super) fn list(&self, params: &Value) -> Result<Value, Refused> {
        let wanted = params["cwd"].as_str();
        if let Some(cwd) = wanted
            && !Path::new(cwd).is_absolute()
        {
            return Err(Refused::params(format!(
                "cwd must be an absolute path: {cwd}"
            )));
        }
        let open = lock(&self.table);
        let sessions: Vec<Value> = self
            .host
            .threads
            .list(wanted)
            .into_iter()
            .filter(|(_, id)| !open.contains_key(id))
            .map(|(cwd, id)| json!({ "sessionId": id, "cwd": cwd }))
            .collect();
        Ok(json!({ "sessions": sessions }))
    }

    pub(super) fn cancel(&self, params: &Value) {
        // An unknown session is nothing to cancel.
        let Ok(id) = session_id(params) else { return };
        if let Some(session) = lock(&self.table).get(id)
            && let Some((cancel, _)) = &session.turn
        {
            cancel.cancel();
        }
    }

    pub(super) async fn close(&self, params: &Value) -> Result<Value, Refused> {
        let id = session_id(params)?;
        let session = lock(&self.table)
            .remove(id)
            .ok_or_else(|| Refused::params(format!("unknown session: {id}")))?;
        if let Some((cancel, turn)) = session.turn {
            cancel.cancel();
            let _ = turn.await;
        }
        Ok(json!({}))
    }

    pub(super) async fn shut_down(&self) {
        let sessions: Vec<Session> = lock(&self.table).drain().map(|(_, s)| s).collect();
        for session in sessions {
            if let Some((cancel, turn)) = session.turn {
                cancel.cancel();
                let _ = turn.await;
            }
        }
    }

    /// Starts a turn; its response goes out when it ends. One at a time per session.
    pub(super) fn prompt(&self, request: Value, params: &Value) -> Result<(), Refused> {
        let id = session_id(params)?.to_owned();
        let text = updates::prompt_text(&params["prompt"]).map_err(Refused::params)?;
        let mut table = lock(&self.table);
        let session = table
            .get_mut(&id)
            .ok_or_else(|| Refused::params(format!("unknown session: {id}")))?;
        if session.history.is_none() {
            return Err(Refused::params(
                "a prompt is already in flight for this session",
            ));
        }
        let lease = self.host.threads.lease(&id).map_err(Refused::internal)?;
        let Some(held) = session.history.take() else {
            return Err(Refused::internal("the conversation is not here"));
        };
        // Under the lease the saved conversation is the truth: the app may have added to it since the last prompt.
        let before = self.host.threads.load(&id);
        let mut history = before.as_ref().map_or(held, |saved| saved.history.clone());
        history.push(Message::new(Role::User, text));
        let cancel = CancellationToken::new();
        let turn = Turn {
            id,
            request,
            agent: Arc::clone(&session.agent),
            built: (session.provider.clone(), session.model.clone()),
            folder: session.folder.clone(),
            host: self.host.clone(),
            before,
            client: self.client.clone(),
            table: Arc::clone(&self.table),
            cancel: cancel.clone(),
        };
        let task = tokio::spawn(turn.run(history, lease));
        session.turn = Some((cancel, task));
        Ok(())
    }
}

struct Turn {
    id: String,
    request: Value,
    agent: Arc<Agent>,
    built: (String, String),
    folder: String,
    host: Host,
    /// The conversation as it was saved when the turn began.
    before: Option<SavedThread>,
    client: Client,
    table: Table,
    cancel: CancellationToken,
}

impl Turn {
    /// Asks the client whether a call may run. A cancelled turn, an error or silence from a gone client is a no.
    fn ask(&self, call: &str, title: &str) {
        let answer = self.client.request(
            "session/request_permission",
            &json!({
                "sessionId": self.id,
                "toolCall": { "toolCallId": call, "title": title, "status": "pending" },
                "options": updates::permission_options(),
            }),
        );
        let (agent, cancel) = (Arc::clone(&self.agent), self.cancel.clone());
        let call = call.to_owned();
        tokio::spawn(async move {
            let allow = tokio::select! {
                () = cancel.cancelled() => false,
                response = answer => response.is_ok_and(|r| updates::allowed(&r)),
            };
            agent.answer(&call, allow);
        });
    }

    async fn run(self, mut history: Vec<Message>, lease: Box<dyn Send>) {
        let (events, mut seen) = mpsc::unbounded_channel();
        let (agent, cancel) = (Arc::clone(&self.agent), self.cancel.clone());
        let work = async {
            let outcome = agent
                .run_until(&mut history, &events, Thinking::default(), &cancel)
                .await;
            drop(events);
            outcome
        };
        let tell = async {
            let mut last = None;
            while let Some(event) = seen.recv().await {
                if let AgentEvent::Approval { id, title } = &event {
                    self.ask(id, title);
                } else if let Some(update) = updates::update(&event) {
                    self.client.update(&self.id, &update);
                }
                if matches!(
                    event,
                    AgentEvent::Done | AgentEvent::Stopped | AgentEvent::StepLimit
                ) {
                    last = Some(event);
                }
            }
            last
        };
        let (outcome, last) = tokio::join!(work, tell);
        let saved = self.host.threads.save(&record(
            &self.id,
            &self.folder,
            (&self.built.0, &self.built.1),
            &history,
            self.before.as_ref(),
        ));
        drop(lease);
        if let Some(session) = lock(&self.table).get_mut(&self.id) {
            session.history = Some(history);
        }
        let result = match (outcome, saved) {
            (Err(e), _) => Err(Refused::internal(e.to_string())),
            (Ok(()), Err(e)) => Err(Refused::internal(format!(
                "the turn ran but was not saved: {e}"
            ))),
            (Ok(()), Ok(())) => Ok(json!({ "stopReason": updates::stop_reason(last.as_ref()) })),
        };
        self.client.answer(&self.request, &result);
    }
}
