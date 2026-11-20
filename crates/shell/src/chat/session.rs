//! Connects the UI to whatever answers: Vivianne's own agent loop, or a coding agent (Claude Code, Codex...)
//! run through the Agent Client Protocol. The work runs on a tokio runtime; the UI watches its events on a
//! channel (no GPUI types here).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Instant;

use serde_json::json;
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use vivianne_ai::ai::types::{CostBreakdown, Usage};
use vivianne_ai::inference::usage_store;
use vivianne_core::{Message, Role, Thinking};
use vivianne_providers::acp::{Acp, AcpEvent, Line, Outcome, PermissionRequest, PromptPart, TurnRequest};
use vivianne_runtime::{Agent, AgentEvent};

static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);

/// When this run of the app started, in seconds. Thread names carry it, so a thread of this run is never
/// mistaken for one of an earlier run that had the same number: usage is kept per thread name, for days.
static LAUNCH: LazyLock<u64> =
    LazyLock::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()));

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
            thread: format!("thread-{:x}-{}", *LAUNCH, NEXT_THREAD.fetch_add(1, Ordering::SeqCst)),
            history: Some(Vec::new()),
            before_run: None,
            task: None,
            stop: None,
            approvals: Approvals::default(),
        }
    }

    /// The name usage is recorded under (`main:<this>`).
    #[must_use]
    pub fn thread_id(&self) -> &str {
        &self.thread
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
                let (events_tx, watcher) = watch_turn(self.runtime.handle(), self.thread.clone(), events_tx);
                self.task = Some(self.runtime.spawn(async move {
                    let started = Instant::now();
                    let result = agent.run(&mut history, &events_tx, thinking).await;
                    drop(events_tx);
                    let error = result.err().map(|e| e.to_string());
                    watcher.finish(started, error.as_deref()).await;
                    let _ = done_tx.send(Finished { history, error });
                }));
            }
            Engine::Harness { id, acp, workspace } => {
                let stop = CancellationToken::new();
                self.stop = Some(stop.clone());
                // What was said before this message, for an agent that wasn't there for it.
                let earlier: Vec<Line> = self.before_run.as_deref().map(lines_of).unwrap_or_default();
                let (events_tx, watcher) = watch_turn(self.runtime.handle(), self.thread.clone(), events_tx);
                let turn = HarnessTurn {
                    acp: acp.clone(),
                    id: id.clone(),
                    thread: self.thread.clone(),
                    cwd: workspace.display().to_string(),
                    text,
                    earlier,
                    approvals: self.approvals.clone(),
                };
                self.task = Some(self.runtime.spawn(async move {
                    let started = Instant::now();
                    let (reply, error) = turn.run(&events_tx, &stop).await;
                    drop(events_tx);
                    watcher.finish(started, error.as_deref()).await;
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

/// What a turn did, as the watcher saw it go by.
#[derive(Debug, Default)]
struct TurnStats {
    tools: u32,
    /// The provider and model that answered last (a fallback may have answered), from its usage.
    model: Option<(String, String)>,
}

/// Sits between a turn and the chat: records each model call's usage against `thread` (so it is on disk before
/// the chat hears of it), counts tool calls and notes them, and passes every event on. Returns the sender the
/// turn writes to, and the watcher to finish once the turn is over.
fn watch_turn(runtime: &tokio::runtime::Handle, thread: String, chat: mpsc::UnboundedSender<AgentEvent>) -> (mpsc::UnboundedSender<AgentEvent>, Watcher) {
    let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
    let task = runtime.spawn(async move {
        let mut stats = TurnStats::default();
        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::Usage { provider, model, .. } => {
                    stats.model = Some((provider.clone(), model.clone()));
                    record_usage(&thread, &event).await;
                }
                AgentEvent::ToolEnd { name, is_error, .. } => {
                    stats.tools += 1;
                    vivianne_telemetry::track("tool.call", json!({ "tool": name, "ok": !is_error }));
                }
                _ => {}
            }
            // A chat that has gone away must not stop the rest being recorded.
            let _ = chat.send(event);
        }
        stats
    });
    (tx, Watcher(task))
}

/// The watcher's task: ends once every sender is dropped and everything sent has been recorded.
struct Watcher(JoinHandle<TurnStats>);

impl Watcher {
    /// Waits for the last events to be recorded, then notes the finished turn. `error` is why it failed, if it did.
    async fn finish(self, started: Instant, error: Option<&str>) {
        let stats = self.0.await.unwrap_or_default();
        let mut props = json!({
            "outcome": error.map_or("ok", error_kind),
            "ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "tools": stats.tools,
        });
        if let (Some((provider, model)), Some(object)) = (stats.model, props.as_object_mut()) {
            object.insert("provider".to_owned(), json!(provider));
            object.insert("model".to_owned(), json!(model));
        }
        vivianne_telemetry::track("turn.complete", props);
    }
}

/// A coarse, content-free name for why a turn failed. The message itself is never recorded.
fn error_kind(error: &str) -> &'static str {
    let error = error.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|word| error.contains(word));
    if has(&["401", "403", "unauthorized", "api key", "sign in", "signed in", "authentication"]) {
        "auth"
    } else if has(&["429", "rate limit", "quota"]) {
        "rate_limit"
    } else if has(&["timed out", "timeout"]) {
        "timeout"
    } else if has(&["connect", "dns", "network", "unreachable"]) {
        "network"
    } else {
        "error"
    }
}

/// A usage event as what the usage store takes, with the provider and model it names.
fn usage_of(event: &AgentEvent) -> Option<(Usage, &str, &str)> {
    let AgentEvent::Usage { provider, model, input, output, cache_read, cache_write, cost } = event else { return None };
    let clamp = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    let usage = Usage {
        input: clamp(*input),
        output: clamp(*output),
        cache_read: clamp(*cache_read),
        cache_write: clamp(*cache_write),
        total_tokens: clamp(input.saturating_add(*output)),
        cost: cost.map(|total| CostBreakdown { total, ..CostBreakdown::default() }),
    };
    Some((usage, provider, model))
}

/// Records a model call's usage for the day, the model and the thread (`main:<thread>`).
async fn record_usage(thread: &str, event: &AgentEvent) {
    let Some((usage, provider, model)) = usage_of(event) else { return };
    usage_store::record_usage(&usage, provider, model).await;
    usage_store::record_usage_tagged(&usage, provider, model, &format!("main:{thread}")).await;
}

/// One turn with a coding agent, and how its events become the chat's.
struct HarnessTurn {
    acp: Arc<Acp>,
    id: String,
    thread: String,
    cwd: String,
    text: String,
    earlier: Vec<Line>,
    approvals: Approvals,
}

impl HarnessTurn {
    /// Runs the turn, sending its events on. Returns what the agent said and the failure, if any.
    async fn run(self, events: &mpsc::UnboundedSender<AgentEvent>, stop: &CancellationToken) -> (String, Option<String>) {
        let (acp_tx, mut acp_rx) = mpsc::unbounded_channel();
        let prompt = [PromptPart::Text(self.text.clone())];
        let request = TurnRequest { prompt: &prompt, earlier: &self.earlier, plan_mode: false, bypass: false };
        let turn = async {
            let outcome = self.acp.run_turn(&self.thread, &self.id, &self.cwd, &request, &acp_tx, stop).await;
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
        let error = match outcome.map(|end| end.outcome) {
            Ok(Outcome::Finished) => {
                let _ = events.send(AgentEvent::Done);
                None
            }
            Ok(Outcome::Stopped) => {
                let _ = events.send(AgentEvent::Stopped);
                None
            }
            Ok(Outcome::Failed(why)) | Err(why) => Some(why),
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
            AcpEvent::Notice { title, text, is_error } => {
                let id = format!("notice-{}-{plans}", self.thread);
                let args = json!({ "title": title, "severity": if is_error { "error" } else { "info" } });
                vec![
                    AgentEvent::ToolStart { id: id.clone(), name: "notice".to_owned(), args },
                    AgentEvent::ToolEnd { id, name: "notice".to_owned(), output: text, is_error, details: None, args: None },
                ]
            }
            // A plan that was removed leaves nothing to draw.
            AcpEvent::Plan(text) if text.is_empty() => Vec::new(),
            // The plan draws as the checklist tool does.
            AcpEvent::Plan(text) => {
                let id = format!("plan-{}-{plans}", self.thread);
                vec![
                    AgentEvent::ToolStart { id: id.clone(), name: "todo_write".to_owned(), args: json!({}) },
                    AgentEvent::ToolEnd { id, name: "todo_write".to_owned(), output: text, is_error: false, details: None, args: None },
                ]
            }
            AcpEvent::Usage(spent) => vec![AgentEvent::Usage {
                provider: spent.provider,
                model: spent.model,
                input: spent.input,
                output: spent.output,
                cache_read: spent.cache_read,
                cache_write: spent.cache_write,
                cost: spent.cost,
            }],
            AcpEvent::Permission(request) => {
                let title = request.why.clone().unwrap_or_else(|| format!("The coding agent wants to use {}", request.tool.name));
                let id = format!("ask-{}-{}", request.tool.id, NEXT_THREAD.fetch_add(1, Ordering::SeqCst));
                self.approvals.lock().unwrap_or_else(PoisonError::into_inner).insert(id.clone(), request);
                vec![AgentEvent::Approval { id, title }]
            }
        }
    }
}

/// The conversation as what a coding agent that joined late is told: who said what, in words.
fn lines_of(history: &[Message]) -> Vec<Line> {
    history
        .iter()
        .filter_map(|m| match m.role {
            Role::User => Some(Line { who: "User", text: m.text.clone() }),
            Role::Assistant => Some(Line { who: "Assistant", text: m.text.clone() }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_usage_event_becomes_the_stores_usage_and_an_unknown_cost_stays_unknown() {
        let event = |cost| AgentEvent::Usage { provider: "p".into(), model: "m".into(), input: 100, output: 20, cache_read: 3, cache_write: 4, cost };
        let with_cost = event(Some(0.5));
        let (usage, provider, model) = usage_of(&with_cost).unwrap();
        assert_eq!((usage.input, usage.output, usage.cache_read, usage.cache_write, usage.total_tokens), (100, 20, 3, 4, 120));
        assert_eq!(usage.cost.map(|c| c.total), Some(0.5));
        assert_eq!((provider, model), ("p", "m"));
        let unknown = event(None);
        assert_eq!(usage_of(&unknown).unwrap().0.cost, None);
        assert!(usage_of(&AgentEvent::Done).is_none());
    }

    #[test]
    fn a_failed_turn_is_noted_by_kind_and_never_by_message() {
        assert_eq!(error_kind("401 Unauthorized: bad key sk-secret"), "auth");
        assert_eq!(error_kind("429 Too Many Requests"), "rate_limit");
        assert_eq!(error_kind("request timed out"), "timeout");
        assert_eq!(error_kind("something odd in /home/me/project"), "error");
    }

    #[tokio::test]
    async fn the_watcher_passes_every_event_on_and_counts_the_tools() {
        let (chat, mut seen) = mpsc::unbounded_channel();
        let (tx, watcher) = watch_turn(&tokio::runtime::Handle::current(), "thread-test".to_owned(), chat);
        tx.send(AgentEvent::Text("hi".into())).unwrap();
        tx.send(AgentEvent::Done).unwrap();
        drop(tx);
        let stats = watcher.0.await.unwrap();
        assert_eq!(stats.tools, 0);
        assert_eq!(seen.recv().await, Some(AgentEvent::Text("hi".into())));
        assert_eq!(seen.recv().await, Some(AgentEvent::Done));
        assert_eq!(seen.recv().await, None, "the chat's channel closes when the turn's does");
    }

    #[test]
    fn a_late_agent_is_told_what_the_user_and_assistant_said_and_nothing_else() {
        let history = vec![
            Message::new(Role::User, "hi".to_owned()),
            Message::assistant("hello".to_owned(), Vec::new()),
            Message::tool_result("c1", "output".to_owned()),
        ];
        let lines = lines_of(&history);
        assert_eq!(lines, vec![Line { who: "User", text: "hi".into() }, Line { who: "Assistant", text: "hello".into() }]);
    }
}
