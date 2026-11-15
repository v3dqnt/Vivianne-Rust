//! Connects the UI to the agent runtime. The agent runs on a tokio runtime;
//! the UI watches its events on a channel (no GPUI types here).

use std::sync::Arc;

use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use vivianne_core::{Message, Role};
use vivianne_runtime::{Agent, AgentEvent};

/// The conversation and the agent that answers it.
pub struct Session {
    agent: Arc<Agent>,
    runtime: Arc<Runtime>,
    /// `None` while a run owns it.
    history: Option<Vec<Message>>,
    /// The history as it was before the current run, restored if the run is stopped.
    before_run: Option<Vec<Message>>,
    task: Option<JoinHandle<()>>,
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
            agent: Arc::new(agent),
            runtime,
            history: Some(Vec::new()),
            before_run: None,
            task: None,
        }
    }

    /// Start answering `text`. `None` if a run is already going.
    pub fn send(&mut self, text: String) -> Option<Run> {
        let mut history = self.history.take()?;
        self.before_run = Some(history.clone());
        history.push(Message::new(Role::User, text));
        let (events_tx, events) = mpsc::unbounded_channel();
        let (done_tx, done) = oneshot::channel();
        let agent = self.agent.clone();
        self.task = Some(self.runtime.spawn(async move {
            let result = agent.run(&mut history, &events_tx).await;
            drop(events_tx);
            let error = result.err().map(|e| e.to_string());
            let _ = done_tx.send(Finished { history, error });
        }));
        Some(Run { events, done })
    }

    /// Stop the current run. The conversation goes back to how it was before
    /// the run, so the stopped message is not part of what the model sees.
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(before) = self.before_run.take() {
            self.history = Some(before);
        }
    }

    /// Take back the history after a run.
    pub fn finish(&mut self, finished: Finished) -> Option<String> {
        self.history = Some(finished.history);
        self.before_run = None;
        self.task = None;
        finished.error
    }
}
