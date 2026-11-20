//! Connects the UI to whatever answers: Vivianne's own agent loop, or a coding agent (Claude Code, Codex...)
//! run through the Agent Client Protocol. The work runs on a tokio runtime; the UI watches its events on a
//! channel (no GPUI types here).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Role, Thinking};
use vivianne_providers::acp::{Acp, AcpEvent, Outcome, PermissionRequest, PromptPart};
use vivianne_runtime::{Agent, AgentEvent};

static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

/// What answers the conversation.
enum Engine {
    /// Vivianne's own loop on a chosen model.
    Own(Arc<Agent>),
    /// A coding agent, which runs its own tools in `workspace`.
    Harness { id: String, acp: Arc<Acp>, workspace: PathBuf },
}

type Approvals = Arc<Mutex<HashMap<String, PermissionRequest>>>;

/// The conversation and what answers it.
pub struct Session {
    engine: Engine,
    runtime: Arc<Runtime>,
    /// Names this conversation to a coding agent, which keeps one session per thread.
    thread: String,
    /// `None` while a run owns it.
    history: Option<Vec<Message>>,
    /// The history as it was before the current run, restored if the run is stopped.
    before_run: Option<Vec<Message>>,
    task: Option<JoinHandle<()>>,
    /// Stops a coding agent's turn (its task is left to finish, so the agent is told to stop).
    stop: Option<CancellationToken>,
    /// Requests from the coding agent waiting for the user.
    approvals: Approvals,
}

/// A started run: events as they happen, then the final state.
pub struct Run {
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    pub done: oneshot::Receiver<Finished>,
}

pub struct Finished {
    history: Vec<Message>,
    pub error: Option<String>,
}

impl Session {
    #[must_use]
    pub fn new(agent: Agent, runtime: Arc<Runtime>) -> Self {
        Self {
            engine: Engine::Own(Arc::new(agent)),
            runtime,
            thread: format!("thread-{}", NEXT_THREAD.fetch_add(1, Ordering::SeqCst)),
            history: Some(Vec::new()),
            before_run: None,
            task: None,
            stop: None,
            approvals: Approvals::default(),
        }
    }

    /// Answer from now on with `agent` (another model); the conversation so far is kept.
    pub fn switch_agent(&mut self, agent: Agent) {
        self.engine = Engine::Own(Arc::new(agent));
    }

    /// Answer from now on with the coding agent `id`, working in `workspace`.
    pub fn use_harness(&mut self, id: &str, acp: Arc<Acp>, workspace: PathBuf) {
        self.engine = Engine::Harness { id: id.to_owned(), acp, workspace };
    }

    /// Start answering `text`. `None` if a run is already going.
    pub fn send(&mut self, text: String, thinking: Thinking) -> Option<Run> {
        let mut history = self.history.take()?;
        self.before_run = Some(history.clone());
        history.push(Message::new(Role::User, text.clone()));
        let (events_tx, events) = mpsc::unbounded_channel();
        let (done_tx, done) = oneshot::channel();
        match &self.engine {
            Engine::Own(agent) => {
                let agent = agent.clone();
                self.task = Some(self.runtime.spawn(async move {
                    let result = agent.run(&mut history, &events_tx, thinking).await;
                    drop(events_tx);
                    let error = result.err().map(|e| e.to_string());
                    let _ = done_tx.send(Finished { history, error });
                }));
            }
            Engine::Harness { id, acp, workspace } => {
                let stop = CancellationToken::new();
                self.stop = Some(stop.clone());
                let turn = HarnessTurn {
                    acp: acp.clone(),
                    id: id.clone(),
                    thread: self.thread.clone(),
                    cwd: workspace.display().to_string(),
                    text,
                    approvals: self.approvals.clone(),
                };
                self.task = Some(self.runtime.spawn(async move {
                    let (reply, error) = turn.run(&events_tx, &stop).await;
                    drop(events_tx);
                    if !reply.is_empty() {
                        history.push(Message::assistant(reply, Vec::new()));
                    }
                    let _ = done_tx.send(Finished { history, error });
                }));
            }
        }
        Some(Run { events, done })
    }

    /// Stop the current run. The conversation goes back to how it was before
    /// the run, so the stopped message is not part of what the model sees.
    pub fn cancel(&mut self) {
        match &self.engine {
            // The coding agent is told to stop and ends its own turn; aborting would leave it working.
            Engine::Harness { .. } => {
                if let Some(stop) = self.stop.take() {
                    stop.cancel();
                }
                self.task = None;
            }
            Engine::Own(_) => {
                if let Some(task) = self.task.take() {
                    task.abort();
                }
            }
        }
        // Anything the agent was still asking is refused.
        self.approvals.lock().unwrap_or_else(PoisonError::into_inner).clear();
        if let Some(before) = self.before_run.take() {
            self.history = Some(before);
        }
    }

    /// Answer a coding agent's request. `false` if it is no longer waiting.
    pub fn answer(&self, id: &str, allow: bool) -> bool {
        let request = self.approvals.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
        request.map(|r| r.answer(allow)).is_some()
    }

    /// Take back the history after a run.
    pub fn finish(&mut self, finished: Finished) -> Option<String> {
        self.history = Some(finished.history);
        self.before_run = None;
        self.task = None;
        self.stop = None;
        finished.error
    }
}

/// One turn with a coding agent, and how its events become the chat's.
struct HarnessTurn {
    acp: Arc<Acp>,
    id: String,
    thread: String,
    cwd: String,
    text: String,
    approvals: Approvals,
}

impl HarnessTurn {
    /// Runs the turn, sending its events on. Returns what the agent said and the failure, if any.
    async fn run(self, events: &mpsc::UnboundedSender<AgentEvent>, stop: &CancellationToken) -> (String, Option<String>) {
        let (acp_tx, mut acp_rx) = mpsc::unbounded_channel();
        let prompt = [PromptPart::Text(self.text.clone())];
        let turn = async {
            let outcome = self.acp.run_turn(&self.thread, &self.id, &self.cwd, &prompt, &acp_tx, stop).await;
            drop(acp_tx);
            outcome
        };
        let forward = async {
            let mut reply = String::new();
            let mut plans = 0_u32;
            while let Some(event) = acp_rx.recv().await {
                if let AcpEvent::Text(piece) = &event {
                    reply.push_str(piece);
                }
                if let AcpEvent::Plan(_) = &event {
                    plans += 1;
                }
                for mapped in self.map(event, plans) {
                    let _ = events.send(mapped);
                }
            }
            reply
        };
        let (outcome, reply) = tokio::join!(turn, forward);
        let error = match outcome {
            Ok(Outcome::Finished) => {
                let _ = events.send(AgentEvent::Done);
                None
            }
            Ok(Outcome::Stopped) => {
                let _ = events.send(AgentEvent::Stopped);
                None
            }
            Ok(Outcome::Refused) => Some("The coding agent declined to continue.".to_owned()),
            Ok(Outcome::Limit(which)) => Some(format!("The coding agent stopped: it hit its limit ({which}).")),
            Err(why) => Some(why),
        };
        (reply, error)
    }

    /// The chat's events for one of the agent's.
    fn map(&self, event: AcpEvent, plans: u32) -> Vec<AgentEvent> {
        match event {
            AcpEvent::Text(piece) => vec![AgentEvent::Text(piece)],
            // Its reasoning is not shown.
            AcpEvent::Thought(_) => Vec::new(),
            AcpEvent::ToolStart { id, name, args } => vec![AgentEvent::ToolStart { id, name, args }],
            AcpEvent::ToolEnd { id, name, args, output, is_error } => {
                vec![AgentEvent::ToolEnd { id, name, output, is_error, details: None, args: Some(args) }]
            }
            // The plan draws as the checklist tool does.
            AcpEvent::Plan(text) => {
                let id = format!("plan-{}-{plans}", self.thread);
                vec![
                    AgentEvent::ToolStart { id: id.clone(), name: "todo_write".to_owned(), args: json!({}) },
                    AgentEvent::ToolEnd { id, name: "todo_write".to_owned(), output: text, is_error: false, details: None, args: None },
                ]
            }
            AcpEvent::Permission(request) => {
                let title = request.why.clone().unwrap_or_else(|| format!("The coding agent wants to use {}", request.tool.name));
                let id = format!("ask-{}-{}", request.tool.id, NEXT_THREAD.fetch_add(1, Ordering::SeqCst));
                self.approvals.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), request);
                vec![AgentEvent::Approval { id, title }]
            }
        }
    }
}
