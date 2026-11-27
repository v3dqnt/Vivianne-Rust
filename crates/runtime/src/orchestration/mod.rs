//! Agents and threads working together: an agent hands a piece of work to a subagent, threads message each other,
//! and a thread can start another (on a coding agent, if that is what it asks for).
//!
//! The tools in [`tools`] and [`threads`] are the model-facing part. What they need from the app is behind two
//! traits the shell fills in at start: a [`Spawner`] that builds a subagent's agent, and a [`ThreadHub`] that
//! reaches the threads the user has open. A tool learns who is calling from [`Caller`], set around every turn, so
//! no tool has to be built per thread.

mod coding;
mod metering;
mod profiles;
mod spawner;
mod subagents;
mod threads;
mod tools;

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use vivianne_core::Mode;
use vivianne_providers::Provider;

use crate::Agent;

pub use coding::{CodingAgents, CodingJob, Installed};
pub use profiles::{Profile, profiles};
pub use spawner::Standard as StandardSpawner;
pub(crate) use subagents::stop_all;
pub use subagents::{Handle, Registry, Status, start_coding};
pub use threads::{AskThread, MessageThread, SpawnThread};
pub use tools::{KillTask, ListSubagentProfiles, Task, TaskOutput, WaitTasks};

tokio::task_local! {
    static CALLER: Caller;
}

/// Who is running a tool right now.
#[derive(Debug, Clone)]
pub struct Caller {
    /// The thread the turn belongs to.
    pub thread: String,
    pub mode: Mode,
    /// The project folder.
    pub folder: PathBuf,
    /// The provider and model the thread answers with, which a subagent uses too.
    pub model: Option<(String, String)>,
    /// The connection that model is reached over, keys and all: a subagent uses the same one.
    pub provider: Option<ParentProvider>,
    /// A sandbox mode is standing.
    pub confined: bool,
    /// How many agents deep: 0 is a thread's own agent, 1 a subagent it started.
    pub depth: usize,
}

/// Runs `work` as `caller`: the tools it calls see who they work for.
pub async fn scope<F: Future>(caller: Caller, work: F) -> F::Output {
    CALLER.scope(caller, work).await
}

/// Who is running this tool, when it is running inside a turn.
#[must_use]
pub fn caller() -> Option<Caller> {
    CALLER.try_with(Clone::clone).ok()
}

/// What a subagent's agent is built to be.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    pub folder: PathBuf,
    /// `Plan` makes it read-only; `Auto` lets it change files.
    pub mode: Mode,
    /// Added to the system prompt: what this kind of subagent is for.
    pub system: String,
    /// The model of the thread that asked, to use unless something else is chosen.
    pub model: Option<(String, String)>,
    /// The parent's connection to it; `None` looks one up by the provider's id.
    pub provider: Option<ParentProvider>,
}

/// A parent's provider, handed to what it starts.
#[derive(Clone)]
pub struct ParentProvider(pub Arc<dyn Provider>);

impl std::fmt::Debug for ParentProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ParentProvider").field(&self.0.id()).finish()
    }
}

/// Builds the agent a subagent runs as. The shell implements it, because it knows the keys and the models.
pub trait Spawner: Send + Sync {
    /// An agent for `spec`, without the tools that start more agents.
    ///
    /// # Errors
    /// The setup is missing something (no model, no key).
    fn build(&self, spec: &AgentSpec) -> Result<Agent, String>;
}

/// What came of a message to another thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// It was idle; the message started it working.
    Woke,
    /// It is busy; the message is the next thing it does.
    Queued,
}

/// A thread another thread started.
#[derive(Debug, Clone)]
pub struct NewThread {
    pub id: String,
    pub title: String,
}

/// The threads the user has open, as the tools reach them.
#[async_trait]
pub trait ThreadHub: Send + Sync {
    /// Gives `to` a message that already says who it is from.
    async fn deliver(&self, to: &str, text: String) -> Result<Delivered, String>;

    /// Delivers, then waits for the thread's next reply and returns it.
    async fn ask(&self, to: &str, text: String, wait: Duration) -> Result<String, String>;

    /// Starts a thread with `prompt`, on the coding agent `agent` when one is named.
    async fn spawn(
        &self,
        title: Option<String>,
        prompt: String,
        agent: Option<String>,
    ) -> Result<NewThread, String>;
}

static SPAWNER: OnceLock<Arc<dyn Spawner>> = OnceLock::new();
static HUB: OnceLock<Arc<dyn ThreadHub>> = OnceLock::new();

/// Says how subagents are built. Once, at start.
pub fn set_spawner(spawner: Arc<dyn Spawner>) {
    let _ = SPAWNER.set(spawner);
}

/// Says how threads are reached. Once, at start.
pub fn set_hub(hub: Arc<dyn ThreadHub>) {
    let _ = HUB.set(hub);
}

pub(crate) fn spawner() -> Option<&'static Arc<dyn Spawner>> {
    SPAWNER.get()
}

pub(crate) fn hub() -> Option<&'static Arc<dyn ThreadHub>> {
    HUB.get()
}

/// A message passed on and on (A asks B, B asks C, ...) stops here.
pub const MAX_HOPS: u32 = 4;
/// A thread started by a thread can go this many levels down.
pub const MAX_DEPTH: u32 = 3;

#[derive(Default)]
struct Chains {
    /// For a thread: how many messages it is downstream of, so a reply that goes on does not go on forever.
    hops: HashMap<String, u32>,
    /// For a thread a thread started: how many threads deep it is.
    depths: HashMap<String, u32>,
}

fn chains() -> &'static Mutex<Chains> {
    static CHAINS: OnceLock<Mutex<Chains>> = OnceLock::new();
    CHAINS.get_or_init(Mutex::default)
}

/// The user said something in `thread`: whatever chain brought it work is over.
pub fn user_turn(thread: &str) {
    chains()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .hops
        .remove(thread);
}

/// The hop a message from `from` to `to` is, and a note that `to` is working on it.
pub(crate) fn next_hop(from: &str, to: &str) -> Result<u32, String> {
    let mut chains = chains().lock().unwrap_or_else(PoisonError::into_inner);
    let hop = chains.hops.get(from).copied().unwrap_or(0) + 1;
    if hop > MAX_HOPS {
        return Err(format!(
            "This message has been passed on {MAX_HOPS} times already. Stop passing it on: answer from what you know, or ask the user."
        ));
    }
    chains.hops.insert(to.to_owned(), hop);
    Ok(hop)
}

/// How deep a thread started by `from` would be; refuses past the limit. Remembers it for `child`.
pub(crate) fn child_depth(from: &str) -> Result<u32, String> {
    let chains = chains().lock().unwrap_or_else(PoisonError::into_inner);
    let depth = chains.depths.get(from).copied().unwrap_or(0) + 1;
    if depth > MAX_DEPTH {
        return Err(format!(
            "Threads started by threads can only go {MAX_DEPTH} levels down. Do this work in this thread."
        ));
    }
    Ok(depth)
}

pub(crate) fn record_depth(child: &str, depth: u32) {
    chains()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .depths
        .insert(child.to_owned(), depth);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_chain_stops_after_its_hops_and_a_user_turn_starts_over() {
        let (a, b, c) = ("hop-a", "hop-b", "hop-c");
        assert_eq!(next_hop(a, b), Ok(1));
        assert_eq!(next_hop(b, c), Ok(2));
        let mut from = c;
        for hop in 3..=MAX_HOPS {
            let to = format!("hop-more-{hop}");
            assert_eq!(next_hop(from, &to), Ok(hop));
            from = Box::leak(to.into_boxed_str());
        }
        assert!(next_hop(from, "hop-end").is_err(), "past the cap");
        user_turn(from);
        assert_eq!(next_hop(from, "hop-end"), Ok(1));
    }

    #[test]
    fn threads_started_by_threads_have_a_depth_limit() {
        record_depth("deep-1", 1);
        record_depth("deep-2", 2);
        record_depth("deep-3", MAX_DEPTH);
        assert_eq!(child_depth("deep-0-root"), Ok(1));
        assert_eq!(child_depth("deep-2"), Ok(3));
        assert!(child_depth("deep-3").is_err());
    }
}
