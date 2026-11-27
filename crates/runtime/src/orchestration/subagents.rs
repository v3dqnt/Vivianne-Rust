//! Subagents: the agents a thread's agent starts for a piece of work. Each runs on its own history in a task,
//! alone in a registry the tools look things up in, so a parent can start several, go on with its own work, and
//! come back for the answers.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Role, Thinking};
use vivianne_providers::Spent;

use super::coding::{self, CodingAgents, CodingJob};
use super::metering;
use super::{AgentSpec, Caller, Spawner};
use crate::{Agent, AgentEvent};

/// Most subagents working at once, across every thread.
pub const MAX_ACTIVE: usize = 6;
/// Longest an answer is that goes back into a parent's context.
const MAX_OUTPUT_CHARS: usize = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running,
    Completed(String),
    Failed(String),
    Cancelled(Option<String>),
}

impl Status {
    #[must_use]
    pub fn is_done(&self) -> bool {
        !matches!(self, Self::Running)
    }
}

/// One subagent.
#[derive(Debug)]
pub struct Handle {
    pub id: String,
    /// What the parent calls it: `agent-1`, or a name it chose.
    pub name: String,
    pub description: String,
    pub profile: String,
    /// The thread that started it: the only one that can see it.
    pub thread: String,
    started: Instant,
    status: Mutex<Status>,
    finished: watch::Sender<bool>,
    pub(super) cancel: CancellationToken,
    tool_calls: AtomicU32,
}

impl Handle {
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn finish(&self, status: Status) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
        self.finished.send_replace(true);
    }

    pub(super) fn count_tool_call(&self) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// Settles it, unless a kill already did and said why.
    pub(super) fn settle(&self, status: Status) {
        if !self.status().is_done() {
            self.finish(status);
        }
    }

    /// Stops it, if it is still working. True when it was.
    pub fn kill(&self, reason: Option<String>) -> bool {
        if self.status().is_done() {
            return false;
        }
        self.cancel.cancel();
        // The driver sees the cancellation and settles it; this settles it first for a caller that looks at once.
        self.finish(Status::Cancelled(reason));
        true
    }

    /// Waits until it is done, or `limit` passes. The status then.
    pub async fn wait(&self, limit: Duration) -> Status {
        let mut done = self.finished.subscribe();
        let _ = tokio::time::timeout(limit, done.wait_for(|d| *d)).await;
        self.status()
    }

    /// One line for a list or a result: name, state, what it is, how long.
    #[must_use]
    pub fn summary(&self) -> String {
        let state = match self.status() {
            Status::Running => "running".to_owned(),
            Status::Completed(_) => "completed".to_owned(),
            Status::Failed(e) => format!("failed: {e}"),
            Status::Cancelled(_) => "cancelled".to_owned(),
        };
        format!(
            "{} ({}): {state}, {} · {} tool calls · {}s",
            self.name,
            self.id,
            self.description,
            self.tool_calls.load(Ordering::Relaxed),
            self.started.elapsed().as_secs()
        )
    }

    /// What it answered, or why it has no answer.
    #[must_use]
    pub fn report(&self) -> String {
        match self.status() {
            Status::Running => format!("{} is still working.", self.name),
            Status::Completed(text) if text.trim().is_empty() => {
                format!("{} finished without saying anything.", self.name)
            }
            Status::Completed(text) => text,
            Status::Failed(e) => format!("{} failed: {e}", self.name),
            Status::Cancelled(reason) => format!(
                "{} was stopped{}.",
                self.name,
                reason.map(|r| format!(": {r}")).unwrap_or_default()
            ),
        }
    }
}

/// Every subagent started in this run of the app.
#[derive(Debug, Default)]
pub struct Registry {
    all: Mutex<Vec<Arc<Handle>>>,
}

impl Registry {
    #[must_use]
    pub fn global() -> &'static Self {
        static REGISTRY: OnceLock<Registry> = OnceLock::new();
        REGISTRY.get_or_init(Self::default)
    }

    fn all(&self) -> Vec<Arc<Handle>> {
        self.all
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// What `thread` started, oldest first.
    #[must_use]
    pub fn of(&self, thread: &str) -> Vec<Arc<Handle>> {
        self.all()
            .into_iter()
            .filter(|h| h.thread == thread)
            .collect()
    }

    /// One of them, by id or by name: a thread cannot reach another thread's subagents by guessing.
    #[must_use]
    pub fn get(&self, thread: &str, id_or_name: &str) -> Option<Arc<Handle>> {
        let mine = self.of(thread);
        mine.iter()
            .rev()
            .find(|h| h.id == id_or_name || h.name == id_or_name)
            .cloned()
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.all().iter().filter(|h| !h.status().is_done()).count()
    }

    /// The next free `agent-N` for `thread`.
    #[must_use]
    pub fn next_name(&self, thread: &str) -> String {
        let used = self.of(thread);
        (1..)
            .map(|n| format!("agent-{n}"))
            .find(|name| !used.iter().any(|h| &h.name == name))
            .unwrap_or_default()
    }
}

/// Stops every subagent `thread` still has working: what the person stopped does not go on behind their back.
pub(crate) fn stop_all(thread: &str, reason: &str) {
    for handle in Registry::global().of(thread) {
        handle.kill(Some(reason.to_owned()));
    }
}

/// A name the parent may choose: lower-case, starts with a letter, up to 32 characters, never `lead`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    name != "lead"
        && name.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// What to start.
pub struct Start {
    pub name: String,
    pub description: String,
    pub profile: String,
    pub prompt: String,
    pub spec: AgentSpec,
}

/// A new running entry for `caller`, if there is room for one.
fn register(
    caller: &Caller,
    name: String,
    description: String,
    profile: String,
) -> Result<Arc<Handle>, String> {
    let registry = Registry::global();
    let mut all = registry.all.lock().unwrap_or_else(PoisonError::into_inner);
    if all.iter().filter(|h| !h.status().is_done()).count() >= MAX_ACTIVE {
        return Err(format!(
            "{MAX_ACTIVE} subagents are already working. Wait for some to finish (task_output, wait_tasks) or stop one (kill_task) first."
        ));
    }
    let (finished, _) = watch::channel(false);
    let handle = Arc::new(Handle {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        description,
        profile,
        thread: caller.thread.clone(),
        started: Instant::now(),
        status: Mutex::new(Status::Running),
        finished,
        cancel: CancellationToken::new(),
        tool_calls: AtomicU32::new(0),
    });
    all.push(handle.clone());
    Ok(handle)
}

/// Starts a subagent for the caller and returns at once; it works in the background.
///
/// # Errors
/// Too many are working, or its agent cannot be built.
pub fn start(
    spawner: &dyn Spawner,
    caller: &Caller,
    request: Start,
) -> Result<Arc<Handle>, String> {
    let agent = spawner.build(&request.spec)?;
    let handle = register(caller, request.name, request.description, request.profile)?;
    // One level down: what it calls sees itself as a subagent and cannot start more.
    let child = Caller {
        depth: caller.depth + 1,
        mode: request.spec.mode,
        member: Some(super::Member {
            id: handle.id.clone(),
            name: handle.name.clone(),
        }),
        ..caller.clone()
    };
    let driver = handle.clone();
    tokio::spawn(super::scope(child, drive(agent, request.prompt, driver)));
    Ok(handle)
}

/// [`start`] for a coding agent from the registry.
///
/// # Errors
/// Too many are working.
pub fn start_coding(
    agents: Arc<dyn CodingAgents>,
    caller: &Caller,
    name: String,
    description: String,
    job: CodingJob,
) -> Result<Arc<Handle>, String> {
    let handle = register(caller, name, description, job.agent.clone())?;
    tokio::spawn(coding::drive(agents, job, handle.clone()));
    Ok(handle)
}

async fn drive(agent: Agent, prompt: String, handle: Arc<Handle>) {
    let mut history = vec![Message::new(Role::User, prompt)];
    let (events, mut seen) = mpsc::unbounded_channel();
    let counter = handle.clone();
    let thread = handle.thread.clone();
    let agent = Arc::new(agent);
    let asker = agent.clone();
    let counting = tokio::spawn(async move {
        while let Some(event) = seen.recv().await {
            match event {
                AgentEvent::ToolStart { .. } => counter.count_tool_call(),
                AgentEvent::Usage {
                    provider,
                    model,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost,
                } => {
                    let spent = Spent {
                        provider,
                        model,
                        input,
                        output,
                        cache_read,
                        cache_write,
                        cost,
                    };
                    metering::record(&thread, &spent).await;
                }
                // Nobody is watching to say yes: a subagent that would have to ask is told no.
                AgentEvent::Approval { id, .. } => {
                    asker.answer(&id, false);
                }
                _ => {}
            }
        }
    });
    let result = tokio::select! {
        run = agent.run(&mut history, &events, Thinking::Medium) => Some(run),
        () = handle.cancel.cancelled() => None,
    };
    drop(events);
    let _ = counting.await;
    let status = match result {
        None => Status::Cancelled(None),
        Some(Err(e)) => Status::Failed(e.to_string()),
        Some(Ok(())) => Status::Completed(last_answer(&history)),
    };
    handle.settle(status);
}

/// What the subagent last said, cut to what a parent's context can take.
fn last_answer(history: &[Message]) -> String {
    let text = history
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.text.trim().is_empty())
        .map(|m| m.text.trim().to_owned())
        .unwrap_or_default();
    cut(&text)
}

/// `text`, no longer than a parent's context can take.
pub(super) fn cut(text: &str) -> String {
    if text.chars().count() <= MAX_OUTPUT_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_OUTPUT_CHARS).collect();
    format!("{cut}\n… (the rest is left out)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_lower_case_short_and_not_lead() {
        assert!(valid_name("scout-2"));
        assert!(
            !valid_name("Scout")
                && !valid_name("2scout")
                && !valid_name("lead")
                && !valid_name(&"a".repeat(33))
        );
    }

    #[test]
    fn the_answer_is_the_last_thing_the_assistant_said() {
        let history = vec![
            Message::new(Role::User, "go"),
            Message::assistant("first", Vec::new()),
            Message::assistant("", Vec::new()),
        ];
        assert_eq!(last_answer(&history), "first");
        assert_eq!(last_answer(&[]), "");
    }
}
