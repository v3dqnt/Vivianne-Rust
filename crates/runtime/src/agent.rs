//! The turn loop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Mode, Role, Thinking, Timing, ToolCall};
use vivianne_providers::{
    ChatEvent, ChatRequest, EventStream, Provider, ProviderError, Spent, ToolSpec,
};
use vivianne_tools::{ToolResult, Toolbox};

use crate::AgentEvent;
use crate::attempts::Attempts;
use crate::repeats::Repeats;
use crate::compact;
use crate::context::{self, Breakdown};
use crate::history;
use crate::resilience::{Chain, Failure, RetryPolicy, Stopped, retry};
use crate::sections::{Sections, order};
use crate::workspace_context::{WorkspaceContext, touched_paths};

/// Same ceiling the old app used for chats.
const DEFAULT_MAX_STEPS: usize = 100;

/// Longest the call that writes a checkpoint may take.
const SUMMARY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// Longest the call that writes a title may take: the reply never waits for it, but a slow one is not worth keeping.
const TITLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// What a tool that was cut short tells the model, so every call it made has an answer.
/// Most tool calls of one step that run at the same time.
const MAX_PARALLEL_TOOLS: usize = 10;

const CANCELLED_TOOL: &str = "error: cancelled by the user before this finished";

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The conversation could not be made durable, so nothing was sent and no tool ran (see [`Checkpoint`]).
    #[error("could not save the conversation before going on: {0}")]
    Checkpoint(String),
}

/// Makes the conversation as it stands durable. Asked before every model request and before the tools of a step run:
/// if it fails, the model is not asked and the tools do not run, so what happens in the world is never ahead of what
/// was saved. Port of the original's session checkpoint policy.
#[derive(Clone)]
pub struct Checkpoint(Arc<dyn Fn(&[Message]) -> Result<(), String> + Send + Sync>);

impl Checkpoint {
    pub fn new(save: impl Fn(&[Message]) -> Result<(), String> + Send + Sync + 'static) -> Self {
        Self(Arc::new(save))
    }
}

impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Checkpoint")
    }
}

/// One place a request can go: a provider (which holds its own key) and a model on it.
#[derive(Clone)]
pub struct Target {
    pub provider: Arc<dyn Provider>,
    pub model: String,
}

impl Target {
    #[must_use]
    pub fn new(provider: Arc<dyn Provider>, model: &str) -> Self {
        Self {
            provider,
            model: model.to_owned(),
        }
    }

    /// `provider/model`, for telling the user.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}/{}", self.provider.id(), self.model)
    }
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("provider", &self.provider.id())
            .field("model", &self.model)
            .finish()
    }
}

#[derive(Debug)]
pub struct Agent {
    /// The chosen model first, then fallbacks (other models, or the same model on another key).
    targets: Vec<Target>,
    chain: Chain,
    system: Option<String>,
    tools: Toolbox,
    max_steps: usize,
    retry: RetryPolicy,
    /// What the agent is for right now. Set through [`Agent::set_mode`]; read at every step, so a change takes
    /// effect on the next model call.
    mode: Mutex<Mode>,
    /// Tool calls waiting for the user's yes or no, by call id.
    asking: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    /// How many tokens the chosen model can hold, 0 when its provider does not say.
    window: AtomicU32,
    /// What is added to the conversation before each model call: instruction files, the clock, the terminal's place.
    workspace: Option<WorkspaceContext>,
    /// What each run changed, per file, when asked for.
    changes: Option<Arc<crate::changes::Recorder>>,
    /// Longest a tool call may run when the tool names no limit of its own; `None` leaves it to the tool.
    tool_timeout: Option<std::time::Duration>,
    /// Runs on every tool result before the model sees it.
    result_filter: Option<ResultFilter>,
    /// Prompt sections a caller added or replaced, by name.
    sections: Mutex<Sections>,
    /// Where the conversation is made durable mid-run, when something wants that.
    checkpoint: Mutex<Option<Checkpoint>>,
    /// Messages sent while a run is going, added before its next model call.
    inbox: Inbox,
    /// Runs a surface started on its own since the person last said something.
    wakes: AtomicU32,
    /// The conversation this agent answers, for the tools that need to know who calls.
    thread: Mutex<Option<String>>,
    /// The message the last such run was started with.
    woke_with: Mutex<String>,
    /// Puts the end of a background job in the inbox, while this agent lives.
    _notices: vivianne_tools::jobs::Subscription,
    /// Asked for a block of standing context (core memory) at the start of every run.
    context_source: Option<ContextSource>,
    context: Mutex<Option<String>>,
    /// Where attached images and files are kept. Without it a message's attachments are not sent.
    attachments: Option<Arc<vivianne_attachments::Store>>,
}

/// Changes a tool's result after it ran and before the model sees it (to redact, to cut, to annotate).
#[derive(Clone)]
pub struct ResultFilter(Arc<dyn Fn(&ToolCall, ToolResult) -> ToolResult + Send + Sync>);

impl ResultFilter {
    pub fn new(
        filter: impl Fn(&ToolCall, ToolResult) -> ToolResult + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(filter))
    }
}

impl std::fmt::Debug for ResultFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResultFilter")
    }
}

/// Something that can say what the agent should always know, read at the start of every run.
#[derive(Clone)]
pub struct ContextSource(
    Arc<dyn Fn() -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>,
);

impl ContextSource {
    pub fn new(
        source: impl Fn() -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(source))
    }
}

impl std::fmt::Debug for ContextSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ContextSource")
    }
}

/// The first thing a stream said, and the rest of it.
struct Opened {
    first: Option<ChatEvent>,
    rest: EventStream,
    /// When the request went out, and when the first event came back.
    asked: std::time::Instant,
    answered: std::time::Instant,
}

/// How many runs in a row background jobs may start without the person.
const MAX_WAKES: u32 = 10;

/// Stops the thread's subagents when dropped, unless disarmed.
struct StopsSubagents(Option<String>);

impl Drop for StopsSubagents {
    fn drop(&mut self) {
        if let Some(thread) = &self.0 {
            crate::orchestration::stop_all(thread, "the turn was stopped");
        }
    }
}

type Inbox = Arc<Mutex<std::collections::VecDeque<String>>>;

impl Agent {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        model: &str,
        system: Option<String>,
        tools: Toolbox,
    ) -> Self {
        let inbox = Inbox::default();
        let notices = {
            let inbox = Arc::clone(&inbox);
            tools.jobs().on_notice(move |text| {
                inbox
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push_back(text);
            })
        };
        Self {
            targets: vec![Target::new(provider, model)],
            chain: Chain::new(1),
            system,
            tools,
            max_steps: DEFAULT_MAX_STEPS,
            retry: RetryPolicy::default(),
            // A library caller has nobody to ask; an app that has sets `Mode::Ask`.
            mode: Mutex::new(Mode::Auto),
            asking: Mutex::default(),
            window: AtomicU32::new(0),
            workspace: None,
            changes: None,
            tool_timeout: None,
            result_filter: None,
            sections: Mutex::new(Sections::default()),
            inbox,
            wakes: AtomicU32::new(0),
            woke_with: Mutex::default(),
            thread: Mutex::default(),
            _notices: notices,
            checkpoint: Mutex::new(None),
            context_source: None,
            context: Mutex::new(None),
            attachments: None,
        }
    }

    /// Says which conversation this agent answers. Its subagent and schedule tools work only once it is said.
    pub fn set_thread(&self, thread: &str) {
        *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread.to_owned());
    }

    /// Who a tool is told is calling: nothing is changed inside a subagent, which has been told already.
    fn caller(&self) -> Option<crate::orchestration::Caller> {
        if crate::orchestration::caller().is_some() {
            return None;
        }
        let thread = self.thread.lock().unwrap_or_else(PoisonError::into_inner).clone()?;
        Some(crate::orchestration::Caller {
            thread,
            mode: self.mode(),
            folder: self.tools.working_directory().root().to_path_buf(),
            model: Some(self.primary()),
            provider: Some(crate::orchestration::ParentProvider(
                self.targets[0].provider.clone(),
            )),
            confined: self.tools.confinement().is_confined(),
            depth: 0,
        })
    }

    /// Changes what the agent is for: its instructions and the tools it may use.
    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    /// The provider and model the agent answers with first.
    #[must_use]
    pub fn primary(&self) -> (String, String) {
        let target = &self.targets[0];
        (target.provider.id().to_owned(), target.model.clone())
    }

    /// Tells the agent how large the model's context window is, so it can condense the conversation before the
    /// window fills. `None` when unknown: it then only condenses after a provider says a request was too large.
    pub fn set_context_window(&self, tokens: Option<u32>) {
        self.window.store(tokens.unwrap_or(0), Ordering::Relaxed);
    }

    /// The user's answer to an [`AgentEvent::Approval`]. `false` if nothing is waiting on `id` any more.
    pub fn answer(&self, id: &str, allow: bool) -> bool {
        let waiting = self
            .asking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        waiting.is_some_and(|w| w.send(allow).is_ok())
    }

    /// Asks the user whether `call` may run, and waits. A dropped question (the run was stopped) counts as no.
    async fn approved(&self, call: &ToolCall, events: &UnboundedSender<AgentEvent>) -> bool {
        let (tx, rx) = oneshot::channel();
        self.asking
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(call.id.clone(), tx);
        // What the person is asked to allow: what it runs, touches or types.
        let detail = ["command", "path", "text", "signal", "sessionId", "job_id", "task_id"]
            .iter()
            .find_map(|k| call.args[k].as_str())
            .unwrap_or_default();
        send(
            events,
            AgentEvent::Approval {
                id: call.id.clone(),
                title: format!("{} {detail}", call.name).trim().to_owned(),
            },
        );
        rx.await.unwrap_or(false)
    }

    /// `invalid arguments: …` when the model's arguments do not fit the tool's parameter schema. A tool of a connected
    /// MCP server is left to that server, and a tool this box does not know is answered as unknown later.
    fn invalid_arguments(&self, call: &ToolCall) -> Option<String> {
        if call.name.starts_with("mcp__") {
            return None;
        }
        let definition = self
            .tools
            .definitions()
            .into_iter()
            .find(|d| d.name == call.name)?;
        vivianne_tools::validate::invalid_arguments(&definition.parameters, &call.args)
    }

    fn mode(&self) -> Mode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The system prompt as sent: the standing instructions, the project's, then the mode's.
    fn system_prompt(&self, mode: Mode) -> Option<String> {
        let standing = self
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut sections = Sections::default();
        if let Some(system) = &self.system {
            sections.set("system", order::SYSTEM, system.clone());
        }
        if let Some(standing) = standing {
            sections.set("context", order::CONTEXT, standing);
        }
        if let Some(mode) = mode.prompt_section() {
            sections.set("mode", order::MODE, mode);
        }
        if self.tools.working_directory().moved() {
            sections.set(
                "working-directory",
                order::CONTEXT + 50,
                format!(
                    "Current working directory: {}. Relative paths and commands start here; the project's folder is {}.",
                    self.tools.working_directory().get().display(),
                    self.tools.working_directory().root().display()
                ),
            );
        }
        if let Some(policy) = self
            .tools
            .confinement()
            .describe(self.tools.working_directory().root())
        {
            sections.set("sandbox-policy", order::CONTEXT + 40, policy);
        }
        if self.tools.get("job_output").is_some() {
            sections.set(
                "background-jobs",
                order::CONTEXT + 45,
                vivianne_tools::jobs::GUIDANCE,
            );
        }
        // What the caller added or replaced by name wins over the built-in sections.
        sections.merge(&self.sections.lock().unwrap_or_else(PoisonError::into_inner));
        sections.assemble()
    }

    /// Adds a section to the system prompt, or replaces the one of that name (including a built-in one: `system`,
    /// `context`, `mode`). Lower orders come first; see [`order`](crate::sections::order).
    pub fn set_section(&self, name: &str, order: i32, text: impl Into<String>) {
        self.sections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set(name, order, text);
    }

    /// Like [`set_section`](Self::set_section), with text asked for each time the prompt is built.
    pub fn set_dynamic_section(
        &self,
        name: &str,
        order: i32,
        text: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) {
        self.sections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .set_dynamic(name, order, text);
    }

    /// Takes a caller's section back out. A built-in one it replaced comes back.
    pub fn remove_section(&self, name: &str) -> bool {
        self.sections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name)
    }

    /// Chooses where the conversation is saved mid-run (`None` stops saving there). See [`Checkpoint`].
    pub fn set_checkpoint(&self, checkpoint: Option<Checkpoint>) {
        *self
            .checkpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = checkpoint;
    }

    /// Saves the conversation now, if a place was chosen. An error stops the run before anything goes out.
    fn save_now(&self, history: &[Message]) -> Result<(), AgentError> {
        let place = self
            .checkpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match place {
            Some(save) => (save.0)(history).map_err(AgentError::Checkpoint),
            None => Ok(()),
        }
    }

    /// Sends a message into a run that is going: it is added to the conversation before the next model call, and a
    /// run that was about to finish goes on to answer it. Between runs it waits for the next one.
    pub fn steer(&self, text: impl Into<String>) {
        self.inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(text.into());
        self.tools.jobs().steered();
    }

    /// Takes what is waiting to be said (a job that ended while nothing was running), so a surface can start a run
    /// with it. `None` when nothing waits, or after `MAX_WAKES` such runs in a row: the rest waits for the person.
    #[must_use]
    pub fn take_waiting(&self) -> Option<String> {
        if self.wakes.load(Ordering::Relaxed) >= MAX_WAKES {
            return None;
        }
        let waiting: Vec<String> = self
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        let said = (!waiting.is_empty()).then(|| waiting.join("\n\n"))?;
        self.wakes.fetch_add(1, Ordering::Relaxed);
        self.woke_with
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone_from(&said);
        Some(said)
    }

    /// The background jobs started by this agent's tools that have not ended, oldest first.
    #[must_use]
    pub fn background_jobs(&self) -> Vec<vivianne_tools::jobs::JobView> {
        let mut jobs = self.tools.jobs().registry().list(None);
        jobs.retain(|job| !job.status.is_terminal());
        jobs
    }

    /// Whether a background job started by this agent's tools has not ended yet.
    #[must_use]
    pub fn background_running(&self) -> bool {
        !self.background_jobs().is_empty()
    }

    /// Whether a steering message is waiting.
    #[must_use]
    pub fn has_steering(&self) -> bool {
        !self
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_empty()
    }

    fn take_steering(&self, history: &mut Vec<Message>) {
        let waiting: Vec<String> = self
            .inbox
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain(..)
            .collect();
        for text in waiting {
            history.push(Message::new(Role::User, text).at(crate::workspace_context::now_ms()));
        }
    }

    /// Limits how long a tool call may run when the tool names no limit of its own.
    #[must_use]
    pub fn with_tool_timeout(mut self, limit: std::time::Duration) -> Self {
        self.tool_timeout = Some(limit);
        self
    }

    /// Changes every tool result after it ran and before the model sees it.
    #[must_use]
    pub fn with_result_filter(mut self, filter: ResultFilter) -> Self {
        self.result_filter = Some(filter);
        self
    }

    /// The tools the model is offered in `mode`.
    fn tool_specs(&self, mode: Mode) -> Vec<ToolSpec> {
        self.tools
            .definitions()
            .into_iter()
            .filter(|d| mode.allows(&d.name))
            .map(|d| ToolSpec {
                name: d.name,
                description: d.description,
                parameters: d.parameters,
            })
            .collect()
    }

    /// Where the context window goes for `history` under the current mode.
    #[must_use]
    pub fn breakdown(&self, history: &[Message]) -> Breakdown {
        let mode = self.mode();
        let tools: Vec<serde_json::Value> = self
            .tool_specs(mode)
            .into_iter()
            .map(|t| serde_json::json!({ "name": t.name, "description": t.description, "parameters": t.parameters }))
            .collect();
        context::breakdown(
            self.system_prompt(mode).as_deref().unwrap_or_default(),
            &tools,
            history,
        )
    }

    /// Adds the project's instruction files (AGENTS.md, CLAUDE.md, and the user's own in `~/.viv` and `~/.agents`), the
    /// clock and the terminal's place to the conversation as durable messages before each model call. The first folder is
    /// the working directory. An edit to a file lands on the next call, and nothing unchanged is said twice.
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn with_instructions(mut self, folders: Vec<String>) -> Self {
        if let Some(cwd) = folders.first() {
            self.workspace = Some(WorkspaceContext::new(std::path::Path::new(cwd)));
        }
        self
    }

    /// Records what each run changes in the working folder: files with their line counts, and each file's comparison.
    #[must_use]
    pub fn with_changes(mut self, recorder: Arc<crate::changes::Recorder>) -> Self {
        self.changes = Some(recorder);
        self
    }

    /// The recorder of this conversation's runs, if one was attached.
    #[must_use]
    pub fn changes(&self) -> Option<&Arc<crate::changes::Recorder>> {
        self.changes.as_ref()
    }

    /// Tells the clock which time zones the client reported for the open turn (a browser's, or the machine's own).
    pub fn report_time_zones(&self, zones: Vec<String>) {
        if let Some(workspace) = &self.workspace {
            workspace.report_zones(zones);
        }
    }

    /// Adds a block of standing context (such as core memory) to the system prompt, asked for at the start of every
    /// run so a change lands on the next message.
    #[must_use]
    pub fn with_context(mut self, source: ContextSource) -> Self {
        self.context_source = Some(source);
        self
    }

    /// Lets the agent send what is attached to messages: images as pictures, files as a line naming their copy.
    #[must_use]
    pub fn with_attachments(mut self, store: Arc<vivianne_attachments::Store>) -> Self {
        self.attachments = Some(store);
        self
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Where to go when the chosen model fails, in order.
    #[must_use]
    pub fn with_fallbacks(mut self, fallbacks: Vec<Target>) -> Self {
        self.targets.extend(fallbacks);
        self.chain = Chain::new(self.targets.len());
        self
    }

    #[must_use]
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Run until the model stops asking for tools. New messages are appended to
    /// `history`. Dropping the returned future cancels the run.
    pub async fn run(
        &self,
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        thinking: Thinking,
    ) -> Result<(), AgentError> {
        self.run_until(history, events, thinking, &CancellationToken::new())
            .await
    }

    /// [`run`](Self::run) that also stops when `cancel` is triggered. A stopped run leaves `history`
    /// consistent: every tool call the model made has an answer.
    pub async fn run_until(
        &self,
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        thinking: Thinking,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        // A run the person started gives the jobs their wakes back.
        let asked = history.iter().rev().find(|m| m.role == Role::User);
        let woke = self
            .woke_with
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if asked.is_some_and(|m| m.text != woke) {
            self.wakes.store(0, Ordering::Relaxed);
        }
        if let Some(recorder) = &self.changes {
            recorder
                .start(vivianne_context::time::turn_of(history))
                .await;
        }
        // A turn the person stops, by the token or by dropping this future, takes its subagents with it.
        let mut subagents = StopsSubagents(self.caller().map(|c| c.thread));
        let outcome = self.run_loop(history, events, thinking, cancel).await;
        if !cancel.is_cancelled() {
            subagents.0 = None;
        }
        if let Some(recorder) = &self.changes {
            // However the run ended, what it changed so far is recorded.
            let _ = recorder.finish().await;
        }
        outcome
    }

    async fn run_loop(
        &self,
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        thinking: Thinking,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        // The connected MCP servers' tools, read again if a server changed its list.
        self.tools.refresh().await;
        if let Some(source) = &self.context_source {
            let standing = (source.0)().await;
            *self.context.lock().unwrap_or_else(PoisonError::into_inner) = standing;
        }
        let mut attempts = Attempts::default();
        let mut repeats = Repeats::default();
        let mut condensed_for_overflow = false;
        // Where the conversation had moved its working directory, read back from what it did (also after a resume).
        self.tools
            .working_directory()
            .replay(&crate::history::calls_with_outcomes(history));
        // Paths the tools used since the last model call: a folder they reach may hold instructions of its own.
        let mut touched: Vec<String> = Vec::new();
        for step in 1..=self.max_steps {
            // A run that was cut off earlier may have left calls without results, which a provider refuses.
            history::repair(history);
            if self.has_steering() {
                repeats.reset();
            }
            self.take_steering(history);
            let (provider, model) = self.primary();
            self.tools
                .set_step(vivianne_tools::StepInfo { provider, model });
            let made_room = tokio::select! {
                () = cancel.cancelled() => None,
                made = self.make_room(history, events, false) => Some(made),
            };
            if made_room.is_none() {
                send(events, AgentEvent::Stopped);
                return Ok(());
            }
            history::mask_old_results(history);
            if let Some(workspace) = &self.workspace {
                workspace.prepare(history, step, &touched);
            }
            touched.clear();
            // Everything so far (the person's message, the last results) is durable before the model is asked.
            self.save_now(history)?;
            let streamed = tokio::select! {
                () = cancel.cancelled() => None,
                streamed = self.stream_step(history, events, thinking, cancel) => Some(streamed),
            };
            let Some(streamed) = streamed else {
                send(events, AgentEvent::Stopped);
                return Ok(());
            };
            let (text, calls, spent, timing) = match streamed {
                Ok(step) => {
                    condensed_for_overflow = false;
                    step
                }
                // The request was too big for the window: condense the conversation and ask again, once.
                Err(AgentError::Provider(error))
                    if !condensed_for_overflow
                        && Failure::from(&error).is_context_overflow()
                        && self.make_room(history, events, true).await =>
                {
                    condensed_for_overflow = true;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if let Some(spent) = spent {
                send(events, usage_event(spent));
            }
            let finished = calls.is_empty();
            let calls = unique_ids(calls, history.len());
            history.push(
                Message::assistant(text, calls.clone())
                    .at(crate::workspace_context::now_ms())
                    .timed(timing),
            );
            if finished {
                // Something said while the model was answering still gets its answer.
                if self.has_steering() {
                    continue;
                }
                send(events, AgentEvent::Done);
                return Ok(());
            }
            // What the model asked for is durable before any of it happens.
            self.save_now(history)?;
            touched.extend(calls.iter().flat_map(|c| touched_paths(&c.args)));
            let stopped = self
                .run_tools(&calls, history, events, cancel, (&mut attempts, &mut repeats))
                .await;
            if stopped {
                // Nobody will answer what was still being asked.
                self.asking
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clear();
                send(events, AgentEvent::Stopped);
                return Ok(());
            }
        }
        send(events, AgentEvent::StepLimit);
        Ok(())
    }

    /// Runs the step's calls and adds each answer to `history` in the order the model gave them. A run of calls to
    /// tools that only read overlaps (at most [`MAX_PARALLEL_TOOLS`] at once); anything else is a barrier that runs
    /// alone once everything before it has settled. `true` if the run was stopped, in which case the calls that did
    /// not finish are answered as cancelled.
    async fn run_tools(
        &self,
        calls: &[ToolCall],
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        (attempts, repeats): (&mut Attempts, &mut Repeats),
    ) -> bool {
        let mut stopped = false;
        let mut next = 0;
        while next < calls.len() {
            let overlapping = vivianne_core::is_parallel_safe(&calls[next].name);
            let end = if overlapping {
                calls[next..]
                    .iter()
                    .take(MAX_PARALLEL_TOOLS)
                    .take_while(|call| vivianne_core::is_parallel_safe(&call.name))
                    .count()
                    + next
            } else {
                next + 1
            };
            let group = &calls[next..end];
            let answers = if stopped {
                vec![None; group.len()]
            } else {
                futures::future::join_all(group.iter().map(|call| async {
                    tokio::select! {
                        () = cancel.cancelled() => None,
                        answer = self.run_tool(call, events) => Some(answer),
                    }
                }))
                .await
            };
            for (call, answer) in group.iter().zip(answers) {
                let finished = answer.is_some();
                stopped |= !finished;
                let mut answer = answer
                    .unwrap_or_else(|| Message::tool_result(&call.id, CANCELLED_TOOL.to_owned()));
                if finished {
                    if let Some(error) = answer.text.strip_prefix("error: ") {
                        if let Some(note) = attempts.failed(call, error) {
                            answer.text.push_str(&note);
                        }
                    } else {
                        attempts.succeeded(call);
                    }
                    if let Some(note) = repeats.observe(call) {
                        answer.text.push_str(&note);
                    }
                }
                // Stamped when it is added, so a conversation's tool time can be read back from it.
                answer.at = Some(crate::workspace_context::now_ms());
                history.push(answer);
            }
            next = end;
        }
        stopped
    }

    /// One model call, on the first target that answers. Each target is retried for passing failures;
    /// a failure that another target could get past moves on, and every move is announced.
    async fn stream_step(
        &self,
        history: &[Message],
        events: &UnboundedSender<AgentEvent>,
        thinking: Thinking,
        cancel: &CancellationToken,
    ) -> Result<Step, AgentError> {
        let mut last_error: Option<ProviderError> = None;
        let order = self.chain.order();
        for (turn, &index) in order.iter().enumerate() {
            let target = &self.targets[index];
            if let Some(previous) = &last_error {
                let from = self.targets[order[turn - 1]].name();
                send(
                    events,
                    AgentEvent::Fallback {
                        from,
                        to: target.name(),
                        reason: Failure::from(previous).short(),
                    },
                );
            }
            match self
                .open_with_retry(target, history, thinking, events, cancel)
                .await
            {
                Ok(opened) => {
                    self.chain.answered(index);
                    return Ok(collect(opened, events).await?);
                }
                Err(error) => {
                    let failure = Failure::from(&error);
                    last_error = Some(error);
                    if !failure.is_worth_another_target() {
                        break;
                    }
                }
            }
        }
        Err(last_error
            .unwrap_or_else(|| {
                ProviderError::Unavailable("No model is set up to answer.".to_owned())
            })
            .into())
    }

    /// Opens a stream on `target`, retrying passing failures. A failure is only seen as such before
    /// the model has said anything: once text has gone to the user, repeating the call would repeat
    /// that text.
    async fn open_with_retry(
        &self,
        target: &Target,
        history: &[Message],
        thinking: Thinking,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<Opened, ProviderError> {
        let projected = match &self.attachments {
            Some(store) => {
                crate::attachments::project(store, history, target.provider.id(), cancel).await
            }
            None => crate::attachments::Projection {
                messages: history.to_vec(),
                images: Vec::new(),
            },
        };
        let request = self.request(target, &projected.messages, thinking, projected.images);
        let on_retry = |retrying: &crate::resilience::Retrying<'_>| {
            send(
                events,
                AgentEvent::Retrying {
                    attempt: retrying.attempt,
                    max_attempts: retrying.max_attempts,
                    delay: retrying.delay,
                    reason: retrying.reason.short(),
                },
            );
        };
        let outcome = retry(&self.retry, cancel, on_retry, |_| async {
            open(target, &request)
                .await
                .map_err(|error| Failure::from(&error))
        })
        .await;
        match outcome {
            Ok(opened) => Ok(opened),
            Err(Stopped::Failed(failure)) => Err(ProviderError::Failed(failure.message)),
            // The caller's own select ends the step at once; this is only reached if it raced us.
            Err(Stopped::Cancelled) => Err(ProviderError::Failed("cancelled".to_owned())),
        }
    }

    /// Condenses the older part of the conversation into one summary when it fills the window (or, with `force`,
    /// because the provider said it does not fit). `true` if it did. The newest part stays word for word.
    async fn make_room(
        &self,
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        force: bool,
    ) -> bool {
        let window = self.window.load(Ordering::Relaxed);
        let total = compact::tokens_of(history);
        if !force && (window == 0 || (total as f32) < window as f32 * compact::THRESHOLD) {
            return false;
        }
        let basis = if window == 0 { total } else { window };
        let cut = history::pick_cut(
            history,
            (basis as f32 * compact::KEEP) as u32,
            compact::MIN_MESSAGES,
        );
        if cut == 0 {
            return false;
        }
        let Some(summary) = self.summarise(&history[..cut], events).await else {
            return false;
        };
        if !compact::shrinks(&summary, compact::tokens_of(&history[..cut])) {
            return false;
        }
        compact::replace_head(history, cut, &summary);
        send(
            events,
            AgentEvent::Compacted {
                before: total,
                after: compact::tokens_of(history),
            },
        );
        true
    }

    /// Asks the first model for a checkpoint of `part`: once with more of each tool result, once with less (a
    /// call that failed on size gets a second go). `None` if neither gives one.
    async fn summarise(
        &self,
        part: &[Message],
        events: &UnboundedSender<AgentEvent>,
    ) -> Option<String> {
        let target = &self.targets[self.chain.order()[0]];
        for preview in compact::PREVIEW_CHARS {
            let request = ChatRequest {
                model: target.model.clone(),
                system: Some(compact::INSTRUCTION.to_owned()),
                messages: vec![Message::new(
                    Role::User,
                    compact::render_transcript(part, preview),
                )],
                tools: Vec::new(),
                thinking: Thinking::Off,
                images: Vec::new(),
            };
            // What the summariser says is not part of the conversation, so its text goes nowhere.
            let (sink, _unread) = tokio::sync::mpsc::unbounded_channel();
            let asked = async { collect(open(target, &request).await?, &sink).await };
            if let Ok(Ok((text, _, spent, _))) = tokio::time::timeout(SUMMARY_TIMEOUT, asked).await
            {
                if let Some(spent) = spent {
                    send(events, usage_event(spent));
                }
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }

    /// A short title for a conversation that opens with `first_ask`, from the first model. `None` if it does not
    /// answer in time or does not answer with a title; the caller keeps its own cleaned-up sentence then.
    pub async fn title(&self, first_ask: &str) -> Option<String> {
        let target = &self.targets[self.chain.order()[0]];
        let (system, user) = crate::title::title_prompt(first_ask);
        let request = ChatRequest {
            model: target.model.clone(),
            system: Some(system),
            messages: vec![Message::new(Role::User, user)],
            tools: Vec::new(),
            thinking: Thinking::Off,
            images: Vec::new(),
        };
        let (sink, _unread) = tokio::sync::mpsc::unbounded_channel();
        let asked = async { collect(open(target, &request).await?, &sink).await };
        let (text, ..) = tokio::time::timeout(TITLE_TIMEOUT, asked)
            .await
            .ok()?
            .ok()?;
        crate::title::parse_title(&text)
    }

    async fn run_tool(&self, call: &ToolCall, events: &UnboundedSender<AgentEvent>) -> Message {
        send(
            events,
            AgentEvent::ToolStart {
                id: call.id.clone(),
                name: call.name.clone(),
                args: call.args.clone(),
            },
        );
        let mode = self.mode();
        // Not offering a tool is not enough: a model can still ask for one it remembers from earlier in the
        // conversation (or from before the mode changed), so the mode is enforced here too.
        let result = if let Some(invalid) = self.invalid_arguments(call) {
            // Before asking the user to approve a call that could not run anyway.
            ToolResult {
                content: invalid,
                is_error: true,
                details: None,
            }
        } else if !mode.allows(&call.name) {
            ToolResult {
                content: format!("`{}` is not available in {} mode", call.name, mode.label()),
                is_error: true,
                details: None,
            }
        } else if let Err(why) = self.tools.confinement().asked(&call.name, &call.args) {
            ToolResult {
                content: why,
                is_error: true,
                details: None,
            }
        } else if (mode.needs_approval(&call.name) && !runs_without_asking(&call.name)
            // Leaving the standing sandbox mode is asked for whatever the approval mode is.
            || matches!(self.tools.confinement().asked(&call.name, &call.args), Ok(Some(_))))
            && !self.approved(call, events).await
        {
            ToolResult {
                content: "the user did not allow this".to_owned(),
                is_error: true,
                details: None,
            }
        } else {
            if let Some(recorder) = &self.changes {
                // The file is copied before the tool changes it, so both sides of its comparison survive later edits.
                for path in crate::changes::edited_paths(&call.name, &call.args) {
                    recorder.capture(std::path::Path::new(&path)).await;
                }
            }
            let caller = self.caller();
            let run = async {
                let run = self.tools.execute(&call.name, &call.args);
                match caller {
                    Some(caller) => crate::orchestration::scope(caller, run).await,
                    None => run.await,
                }
            };
            let limit = self.tools.timeout_of(&call.name).or(self.tool_timeout);
            let ran = match limit {
                Some(limit) => match tokio::time::timeout(limit, run).await {
                    Ok(done) => done,
                    Err(_) => Some(ToolResult {
                        content: format!(
                            "`{}` did not finish within {} seconds and was stopped",
                            call.name,
                            limit.as_secs().max(1)
                        ),
                        is_error: true,
                        details: None,
                    }),
                },
                None => run.await,
            };
            let result = ran.unwrap_or_else(|| ToolResult {
                content: format!("unknown tool `{}`", call.name),
                is_error: true,
                details: None,
            });
            match &self.result_filter {
                Some(filter) => (filter.0)(call, result),
                None => result,
            }
        };
        let output = if result.is_error {
            format!("error: {}", result.content)
        } else {
            result.content
        };
        send(
            events,
            AgentEvent::ToolEnd {
                id: call.id.clone(),
                name: call.name.clone(),
                output: output.clone(),
                is_error: result.is_error,
                details: result.details,
                args: None,
            },
        );
        Message::tool_result(&call.id, output)
    }

    fn request(
        &self,
        target: &Target,
        history: &[Message],
        thinking: Thinking,
        images: Vec<vivianne_providers::ImagePart>,
    ) -> ChatRequest {
        let mode = self.mode();
        ChatRequest {
            model: target.model.clone(),
            system: self.system_prompt(mode),
            messages: history.to_vec(),
            tools: self.tool_specs(mode),
            thinking,
            images,
        }
    }
}

/// Starts a stream and reads its first event, so a failure that comes back as the first item is
/// seen here and not after the caller has started showing output.
async fn open(target: &Target, request: &ChatRequest) -> Result<Opened, ProviderError> {
    let asked = std::time::Instant::now();
    let mut rest = target.provider.stream(request).await?;
    let first = rest.next().await.transpose()?;
    Ok(Opened {
        first,
        rest,
        asked,
        answered: std::time::Instant::now(),
    })
}

/// What one model call came to: its text, the tool calls it asked for, what it used, and how long it took.
type Step = (String, Vec<ToolCall>, Option<Spent>, Timing);

/// Milliseconds between two moments, saturating.
fn millis_between(from: std::time::Instant, to: std::time::Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_millis()).unwrap_or(u64::MAX)
}

/// Reads a stream to its end, sending text on as it arrives. What the call used is summed, so a stream that
/// reports it in pieces still counts as one call.
async fn collect(
    opened: Opened,
    events: &UnboundedSender<AgentEvent>,
) -> Result<Step, ProviderError> {
    let Opened {
        first,
        rest,
        asked,
        answered,
    } = opened;
    let mut stream = futures::stream::iter(first.map(Ok)).chain(rest);
    let (mut text, mut calls, mut spent) = (String::new(), Vec::new(), None::<Spent>);
    while let Some(event) = stream.next().await {
        match event? {
            ChatEvent::Text(piece) => {
                text.push_str(&piece);
                send(events, AgentEvent::Text(piece));
            }
            ChatEvent::ToolCall(call) => calls.push(call),
            ChatEvent::Usage(used) => match spent.as_mut() {
                Some(total) => total.add(&used),
                None => spent = Some(used),
            },
            ChatEvent::Done => break,
        }
    }
    let timing = Timing {
        first_token_ms: millis_between(asked, answered),
        decode_ms: millis_between(answered, std::time::Instant::now()),
    };
    Ok((text, calls, spent, timing))
}

fn usage_event(spent: Spent) -> AgentEvent {
    AgentEvent::Usage {
        provider: spent.provider,
        model: spent.model,
        input: spent.input,
        output: spent.output,
        cache_read: spent.cache_read,
        cache_write: spent.cache_write,
        cost: spent.cost,
    }
}

/// Gives a call an id of its own when the provider sent none, or the same one as an earlier call in the step:
/// results and approvals are matched by id.
fn unique_ids(mut calls: Vec<ToolCall>, position: usize) -> Vec<ToolCall> {
    let mut seen = std::collections::HashSet::new();
    for (i, call) in calls.iter_mut().enumerate() {
        if call.id.is_empty() || !seen.insert(call.id.clone()) {
            call.id = format!("call_{position}_{i}");
            seen.insert(call.id.clone());
        }
    }
    calls
}

/// A closed channel means nobody is watching; the run still finishes.
fn send(events: &UnboundedSender<AgentEvent>, event: AgentEvent) {
    let _ = events.send(event);
}

/// Whether the user let the desktop or the browser act without asking (their own switches, apart from the mode).
fn runs_without_asking(name: &str) -> bool {
    (vivianne_core::is_computer_tool(name) && vivianne_tools::computer::auto())
        || vivianne_tools::browser::auto_approves(name)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
    use vivianne_core::Role;
    use vivianne_tools::Tool;

    use super::*;

    const RATE_LIMIT: &str = "429 Too Many Requests: rate limit exceeded";
    const ONE_SECOND: Duration = Duration::from_secs(1);

    /// One scripted answer to one call.
    enum Reply {
        Events(Vec<ChatEvent>),
        /// The call itself fails.
        Refuse(&'static str),
        /// The stream opens and its first item is a failure.
        FailFirst(&'static str),
    }

    /// Replays one scripted reply per call and records the requests it saw.
    struct Scripted {
        name: &'static str,
        replies: Mutex<VecDeque<Reply>>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Scripted {
        fn named(name: &'static str, replies: Vec<Reply>) -> Arc<Self> {
            Arc::new(Self {
                name,
                replies: Mutex::new(replies.into()),
                seen: Mutex::default(),
            })
        }

        fn new(replies: Vec<Vec<ChatEvent>>) -> Arc<Self> {
            Self::named("scripted", replies.into_iter().map(Reply::Events).collect())
        }

        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl Provider for Scripted {
        fn id(&self) -> &str {
            self.name
        }
        async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
            self.seen.lock().unwrap().push(request.clone());
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Reply::Events(vec![ChatEvent::Done]));
            match reply {
                Reply::Events(events) => {
                    Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
                }
                Reply::Refuse(why) => Err(ProviderError::Failed(why.to_owned())),
                Reply::FailFirst(why) => Ok(Box::pin(futures::stream::iter([Err(
                    ProviderError::Failed(why.to_owned()),
                )]))),
            }
        }
    }

    struct Echo;

    #[async_trait]
    impl Tool for Echo {
        fn name(&self) -> &'static str {
            "echo"
        }
        async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
            ToolResult {
                content: args["say"].as_str().unwrap_or_default().to_owned(),
                is_error: false,
                details: None,
            }
        }
    }

    /// Never finishes, like a command that hangs.
    struct Hang;

    #[async_trait]
    impl Tool for Hang {
        fn name(&self) -> &'static str {
            "hang"
        }
        async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
            std::future::pending().await
        }
    }

    fn call(id: &str, tool: &str) -> ChatEvent {
        ChatEvent::ToolCall(ToolCall {
            id: id.into(),
            name: tool.into(),
            args: json!({"say": "pong"}),
        })
    }

    fn toolbox() -> Toolbox {
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        tools.add(Hang);
        tools
    }

    fn agent(provider: Arc<Scripted>) -> Agent {
        Agent::new(provider, "m", None, toolbox())
    }

    async fn drain(mut rx: UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut seen = Vec::new();
        while let Some(e) = rx.recv().await {
            seen.push(e);
        }
        seen
    }

    fn user(text: &str) -> Vec<Message> {
        vec![Message::new(Role::User, text)]
    }

    #[tokio::test]
    async fn tool_result_is_fed_back_and_the_loop_ends_on_plain_text() {
        let provider = Scripted::new(vec![
            vec![call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Text("all done".into()), ChatEvent::Done],
        ]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("ping");

        agent(provider.clone())
            .run(&mut history, &tx, Thinking::Low)
            .await
            .unwrap();

        let roles: Vec<Role> = history.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [Role::User, Role::Assistant, Role::Tool, Role::Assistant]
        );
        assert_eq!(history[2].text, "pong");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(provider.seen.lock().unwrap()[1].messages.len(), 3);
        assert!(
            provider
                .seen
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.thinking == Thinking::Low)
        );

        drop(tx);
        let seen = drain(rx).await;
        assert!(matches!(seen.first(), Some(AgentEvent::ToolStart { .. })));
        assert_eq!(seen.last(), Some(&AgentEvent::Done));
    }

    fn spent(input: u64, cost: Option<f64>) -> Spent {
        Spent {
            provider: "p".into(),
            model: "m".into(),
            input,
            output: 10,
            cache_read: 0,
            cache_write: 0,
            cost,
        }
    }

    #[tokio::test]
    async fn every_reply_the_loop_streamed_carries_how_long_it_took() {
        let provider = Scripted::new(vec![vec![ChatEvent::Text("hello".into()), ChatEvent::Done]]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("ping");
        agent(provider)
            .run(&mut history, &tx, Thinking::Low)
            .await
            .unwrap();
        let reply = history
            .iter()
            .find(|m| m.role == Role::Assistant)
            .expect("a reply");
        let timing = reply.timing.expect("the reply was timed");
        assert!(
            timing.first_token_ms < 5_000 && timing.decode_ms < 5_000,
            "{timing:?}"
        );
        let json = serde_json::to_value(reply).expect("serializes");
        assert!(
            json["timing"]["first_token_ms"].is_u64(),
            "kept in the saved thread: {json}"
        );
        let user_json = serde_json::to_value(&history[0]).expect("serializes");
        assert!(
            user_json.get("timing").is_none(),
            "nothing is written where there is nothing to say"
        );
    }

    #[tokio::test]
    async fn each_model_call_reports_what_it_used_once_and_pieces_of_one_call_are_summed() {
        let provider = Scripted::new(vec![
            vec![
                ChatEvent::Usage(spent(100, Some(0.01))),
                call("c1", "echo"),
                ChatEvent::Done,
            ],
            vec![
                ChatEvent::Usage(spent(40, Some(0.01))),
                ChatEvent::Usage(spent(60, None)),
                ChatEvent::Text("ok".into()),
                ChatEvent::Done,
            ],
        ]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("ping");
        agent(provider)
            .run(&mut history, &tx, Thinking::Low)
            .await
            .unwrap();
        drop(tx);

        let used: Vec<_> = drain(rx)
            .await
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::Usage {
                    input,
                    output,
                    cost,
                    ..
                } => Some((input, output, cost)),
                _ => None,
            })
            .collect();
        assert_eq!(
            used,
            [(100, 10, Some(0.01)), (100, 20, None)],
            "the second call's cost is unknown once one piece is"
        );
    }

    #[test]
    fn adding_spend_keeps_a_cost_only_while_every_part_has_one() {
        let mut total = spent(1, Some(0.5));
        total.add(&spent(2, Some(0.25)));
        assert_eq!((total.input, total.cost), (3, Some(0.75)));
        total.add(&spent(3, None));
        assert_eq!((total.input, total.cost), (6, None));
    }

    #[tokio::test]
    async fn plan_mode_adds_its_instruction_and_offers_only_the_tools_that_look() {
        let provider = Scripted::new(vec![vec![ChatEvent::Done]]);
        let agent = Agent::new(
            provider.clone(),
            "m",
            Some("Be brief.".to_owned()),
            toolbox(),
        );
        agent.set_mode(Mode::Plan);
        let (tx, _rx) = unbounded_channel();
        agent
            .run(&mut user("x"), &tx, Thinking::default())
            .await
            .unwrap();
        let seen = provider.seen.lock().unwrap();
        assert!(
            seen[0]
                .system
                .as_deref()
                .is_some_and(|s| s.starts_with("Be brief.") && s.contains("PLAN MODE"))
        );
        assert!(
            seen[0].tools.is_empty(),
            "the test tools (echo, hang) are not look-only tools"
        );
    }

    #[tokio::test]
    async fn plan_mode_refuses_a_tool_even_if_the_model_asks_for_it() {
        let provider = Scripted::new(vec![
            vec![call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Done],
        ]);
        let agent = Agent::new(provider, "m", None, toolbox());
        agent.set_mode(Mode::Plan);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        assert!(
            history[2].text.contains("not available in Plan mode"),
            "{}",
            history[2].text
        );
    }

    /// Ask mode stops at a tool that changes things until the user answers; no answer, or no, means it never runs.
    #[tokio::test]
    async fn ask_mode_waits_for_a_yes_before_a_tool_runs() {
        for (allow, ran) in [(true, true), (false, false)] {
            let provider = Scripted::new(vec![
                vec![call("c1", "echo"), ChatEvent::Done],
                vec![ChatEvent::Done],
            ]);
            let agent = Arc::new(Agent::new(provider, "m", None, toolbox()));
            agent.set_mode(Mode::Ask);
            let (tx, mut rx) = unbounded_channel();
            let task = tokio::spawn({
                let agent = agent.clone();
                async move {
                    let mut history = user("x");
                    agent
                        .run(&mut history, &tx, Thinking::default())
                        .await
                        .unwrap();
                    history
                }
            });
            let id = loop {
                if let Some(AgentEvent::Approval { id, .. }) = rx.recv().await {
                    break id;
                }
            };
            assert!(agent.answer(&id, allow));
            let history = task.await.unwrap();
            assert_eq!(
                history[2].text.contains("did not allow"),
                !ran,
                "{}",
                history[2].text
            );
        }
    }

    /// A conversation of `n` messages of about 100 tokens each, ending on the user's turn.
    fn long_history(n: usize) -> Vec<Message> {
        (0..n)
            .map(|i| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    "x".repeat(400),
                )
            })
            .collect()
    }

    fn said(text: &str) -> Vec<ChatEvent> {
        vec![ChatEvent::Text(text.to_owned()), ChatEvent::Done]
    }

    #[tokio::test]
    async fn a_conversation_that_fills_the_window_is_condensed_before_the_next_call() {
        let provider = Scripted::new(vec![said("SUMMARY"), said("answer")]);
        let agent = Agent::new(provider.clone(), "m", None, toolbox());
        agent.set_context_window(Some(1_000));
        let (tx, rx) = unbounded_channel();
        let mut history = long_history(9);
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        drop(tx);
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "one call to summarise, one to answer");
        assert_eq!(seen[0].system.as_deref(), Some(compact::INSTRUCTION));
        assert!(
            seen[1].messages[0].text.starts_with(compact::OPEN)
                && seen[1].messages[0].text.contains("SUMMARY")
        );
        assert!(
            seen[1].messages.len() < 9,
            "the older messages are gone from what is sent"
        );
        assert!(matches!(history.last(), Some(m) if m.text == "answer"));
        assert!(
            futures::executor::block_on(drain(rx))
                .iter()
                .any(|e| matches!(e, AgentEvent::Compacted { .. }))
        );
    }

    #[tokio::test]
    async fn a_request_the_provider_says_is_too_large_is_condensed_and_asked_again_once() {
        let provider = Scripted::named(
            "scripted",
            vec![
                Reply::Refuse("400: maximum context length is 8192 tokens"),
                Reply::Events(said("SUMMARY")),
                Reply::Events(said("answer")),
            ],
        );
        let agent = Agent::new(provider.clone(), "m", None, toolbox());
        let (tx, _rx) = unbounded_channel();
        let mut history = long_history(9);
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        assert_eq!(provider.calls(), 3);
        assert!(matches!(history.last(), Some(m) if m.text == "answer"));
    }

    #[tokio::test]
    async fn a_history_with_an_unanswered_call_is_repaired_before_it_is_sent() {
        let provider = Scripted::new(vec![said("ok")]);
        let agent = Agent::new(provider.clone(), "m", None, toolbox());
        let (tx, _rx) = unbounded_channel();
        let mut history = vec![
            Message::new(Role::User, "go"),
            Message::assistant(
                "",
                vec![ToolCall {
                    id: "c".into(),
                    name: "echo".into(),
                    args: json!({}),
                }],
            ),
            Message::new(Role::User, "hello?"),
        ];
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        let sent = &provider.seen.lock().unwrap()[0].messages;
        assert_eq!(
            sent[2].tool_call_id.as_deref(),
            Some("c"),
            "the call has an answer right after it"
        );
    }

    #[tokio::test]
    async fn standing_context_is_asked_for_at_the_start_of_a_run_and_joins_the_system_prompt() {
        let provider = Scripted::new(vec![vec![ChatEvent::Done]]);
        let agent = Agent::new(
            provider.clone(),
            "m",
            Some("Be brief.".to_owned()),
            toolbox(),
        )
        .with_context(ContextSource::new(|| {
            Box::pin(async { Some("<core_memory>prefers tabs</core_memory>".to_owned()) })
        }));
        let (tx, _rx) = unbounded_channel();
        agent
            .run(&mut user("x"), &tx, Thinking::default())
            .await
            .unwrap();
        let system = provider.seen.lock().unwrap()[0].system.clone().unwrap();
        assert!(system.starts_with("Be brief.") && system.contains("prefers tabs"));
    }

    #[tokio::test]
    async fn a_title_comes_from_the_model_and_is_none_when_it_does_not_answer_with_one() {
        let agent = Agent::new(
            Scripted::new(vec![said("\"Fix login timeout\".")]),
            "m",
            None,
            toolbox(),
        );
        assert_eq!(
            agent.title("my login times out").await.as_deref(),
            Some("Fix login timeout")
        );
        let agent = Agent::new(
            Scripted::new(vec![said(&"word ".repeat(30))]),
            "m",
            None,
            toolbox(),
        );
        assert_eq!(agent.title("x").await, None);
    }

    #[test]
    fn calls_with_no_id_or_a_repeated_id_get_their_own() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            name: "read".into(),
            args: json!({}),
        };
        let ids: Vec<_> = unique_ids(vec![call(""), call("a"), call("a"), call("")], 7)
            .into_iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(ids, ["call_7_0", "a", "call_7_2", "call_7_3"]);
    }

    #[test]
    fn the_breakdown_follows_the_mode_because_withheld_tools_leave_the_window() {
        let agent = Agent::new(
            Scripted::new(vec![]),
            "m",
            Some("Be brief.".to_owned()),
            Toolbox::coding(std::path::Path::new(".")),
        );
        let all = agent.breakdown(&[]);
        agent.set_mode(Mode::Plan);
        let plan = agent.breakdown(&[]);
        assert!(plan.tools > 0 && plan.tools < all.tools, "{all:?} {plan:?}");
        assert!(
            plan.system > all.system,
            "the mode's instruction is part of the system prompt"
        );
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_to_the_model_not_fatal() {
        let provider = Scripted::new(vec![vec![call("c1", "nope")], vec![ChatEvent::Done]]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        agent(provider)
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        assert!(history[2].text.starts_with("error: unknown tool"));
    }

    #[tokio::test]
    async fn step_ceiling_stops_a_model_that_never_finishes() {
        let looping = (0..5).map(|_| vec![call("c", "echo")]).collect();
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        agent(Scripted::new(looping))
            .with_max_steps(3)
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        drop(tx);
        assert_eq!(drain(rx).await.last(), Some(&AgentEvent::StepLimit));
    }

    /// Writes `content` to `path` inside its folder, like the real `write` tool.
    struct Writer(std::path::PathBuf);

    #[async_trait]
    impl Tool for Writer {
        fn name(&self) -> &'static str {
            "write"
        }
        async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
            let path = self.0.join(args["path"].as_str().unwrap_or_default());
            let ok = std::fs::write(path, args["content"].as_str().unwrap_or_default()).is_ok();
            ToolResult {
                content: "written".into(),
                is_error: !ok,
                details: None,
            }
        }
    }

    /// Records when its calls start and end, and how many overlapped.
    struct Probe {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        active: Arc<std::sync::atomic::AtomicUsize>,
        most: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl Tool for Probe {
        fn name(&self) -> &str {
            self.name
        }
        async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
            use std::sync::atomic::Ordering;
            let id = args["id"].as_str().unwrap_or_default().to_owned();
            self.log.lock().unwrap().push(format!("start:{id}"));
            let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.most.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(60)).await;
            self.active.fetch_sub(1, Ordering::SeqCst);
            self.log.lock().unwrap().push(format!("end:{id}"));
            ToolResult {
                content: format!("done {id}"),
                is_error: false,
                details: None,
            }
        }
    }

    fn probe_call(id: &str, tool: &str) -> ChatEvent {
        ChatEvent::ToolCall(ToolCall {
            id: id.into(),
            name: tool.into(),
            args: json!({ "id": id, "path": id, "content": "" }),
        })
    }

    #[tokio::test]
    async fn reads_overlap_a_change_waits_for_them_and_answers_stay_in_the_models_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let (active, most) = (
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let mut tools = Toolbox::new(std::path::Path::new("."));
        for name in ["read", "write"] {
            tools.add(Probe {
                name,
                log: log.clone(),
                active: active.clone(),
                most: most.clone(),
            });
        }
        let provider = Scripted::new(vec![
            vec![
                probe_call("a", "read"),
                probe_call("b", "read"),
                probe_call("c", "read"),
                probe_call("w", "write"),
                probe_call("d", "read"),
                ChatEvent::Done,
            ],
            vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        Agent::new(provider, "m", None, tools)
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        assert!(
            most.load(std::sync::atomic::Ordering::SeqCst) >= 3,
            "the three reads ran together"
        );
        let log = log.lock().unwrap().clone();
        let at = |entry: &str| {
            log.iter()
                .position(|l| l == entry)
                .unwrap_or_else(|| panic!("{entry} in {log:?}"))
        };
        assert!(
            at("start:w") > at("end:a")
                && at("start:w") > at("end:b")
                && at("start:w") > at("end:c"),
            "{log:?}"
        );
        assert!(
            at("start:d") > at("end:w"),
            "the read after the change waits for it: {log:?}"
        );
        let answers: Vec<&str> = history
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(answers, ["done a", "done b", "done c", "done w", "done d"]);
    }

    /// A tool that declares what it needs, so its arguments can be checked.
    struct Strict(Arc<Mutex<usize>>);

    #[async_trait]
    impl Tool for Strict {
        fn name(&self) -> &str {
            "strict"
        }
        fn definition(&self) -> Option<Value> {
            Some(json!({
                "name": "strict",
                "description": "needs a count",
                "parameters": { "type": "object", "properties": { "count": { "type": "integer" } }, "required": ["count"] }
            }))
        }
        async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
            *self.0.lock().unwrap() += 1;
            ToolResult {
                content: "ran".into(),
                is_error: false,
                details: None,
            }
        }
    }

    #[tokio::test]
    async fn arguments_that_do_not_fit_the_schema_come_back_as_an_error_and_the_tool_never_runs() {
        let ran = Arc::new(Mutex::new(0));
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Strict(ran.clone()));
        let bad = ChatEvent::ToolCall(ToolCall {
            id: "x".into(),
            name: "strict".into(),
            args: json!({ "count": "three" }),
        });
        let missing = ChatEvent::ToolCall(ToolCall {
            id: "y".into(),
            name: "strict".into(),
            args: json!({}),
        });
        let good = ChatEvent::ToolCall(ToolCall {
            id: "z".into(),
            name: "strict".into(),
            args: json!({ "count": 3 }),
        });
        let provider = Scripted::new(vec![
            vec![bad, missing, good, ChatEvent::Done],
            vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        Agent::new(provider, "m", None, tools)
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        let answers: Vec<&str> = history
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.text.as_str())
            .collect();
        assert!(
            answers[0].starts_with("error: invalid arguments:")
                && answers[0].contains("\"count\" must be an integer"),
            "{answers:?}"
        );
        assert!(
            answers[1].contains("missing required property \"count\""),
            "{answers:?}"
        );
        assert_eq!(answers[2], "ran");
        assert_eq!(*ran.lock().unwrap(), 1, "only the call that fit ran");
    }

    /// Takes its time, then answers.
    struct Sleepy(&'static str, Option<Duration>, Duration);

    #[async_trait]
    impl Tool for Sleepy {
        fn name(&self) -> &str {
            self.0
        }
        fn timeout(&self) -> Option<Duration> {
            self.1
        }
        fn definition(&self) -> Option<Value> {
            Some(
                json!({ "name": self.0, "description": "waits", "parameters": { "type": "object", "properties": {} } }),
            )
        }
        async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
            tokio::time::sleep(self.2).await;
            ToolResult {
                content: "finished".into(),
                is_error: false,
                details: None,
            }
        }
    }

    fn bare_call(id: &str, tool: &str) -> ChatEvent {
        ChatEvent::ToolCall(ToolCall {
            id: id.into(),
            name: tool.into(),
            args: json!({}),
        })
    }

    async fn run_with(agent: Agent, history: &mut Vec<Message>) {
        let (tx, _rx) = unbounded_channel();
        agent
            .run(history, &tx, Thinking::default())
            .await
            .expect("runs");
    }

    fn tool_answers(history: &[Message]) -> Vec<String> {
        history
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.text.clone())
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_that_outlasts_its_tools_own_limit_or_the_agents_is_stopped_with_an_error() {
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Sleepy(
            "own",
            Some(Duration::from_secs(2)),
            Duration::from_secs(60),
        ));
        tools.add(Sleepy("default", None, Duration::from_secs(60)));
        tools.add(Sleepy(
            "quick",
            Some(Duration::from_secs(2)),
            Duration::from_secs(1),
        ));
        let provider = Scripted::new(vec![
            vec![
                bare_call("a", "own"),
                bare_call("b", "default"),
                bare_call("c", "quick"),
                ChatEvent::Done,
            ],
            vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let mut history = user("go");
        run_with(
            Agent::new(provider, "m", None, tools).with_tool_timeout(Duration::from_secs(10)),
            &mut history,
        )
        .await;
        let answers = tool_answers(&history);
        assert!(
            answers[0].starts_with("error:")
                && answers[0].contains("did not finish within 2 seconds"),
            "{answers:?}"
        );
        assert!(
            answers[1].contains("did not finish within 10 seconds"),
            "{answers:?}"
        );
        assert_eq!(answers[2], "finished");
    }

    #[tokio::test]
    async fn a_result_filter_changes_what_the_model_sees_and_leaves_failures_to_the_filter() {
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        let provider = Scripted::new(vec![
            vec![call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let filter = ResultFilter::new(|call, mut result| {
            result.content = format!("[{}] {}", call.name, result.content.replace("pong", "***"));
            result
        });
        let mut history = user("go");
        run_with(
            Agent::new(provider, "m", None, tools).with_result_filter(filter),
            &mut history,
        )
        .await;
        assert_eq!(tool_answers(&history), ["[echo] ***"]);
    }

    #[tokio::test]
    async fn a_tool_with_a_condition_is_offered_and_callable_only_while_it_holds() {
        let build = |model: &str| {
            let mut tools = Toolbox::new(std::path::Path::new("."));
            tools.add(Echo);
            tools.add_when(
                Sleepy("big_only", None, Duration::from_millis(1)),
                Arc::new(|step: &vivianne_tools::StepInfo| step.model == "big"),
            );
            let provider = Scripted::new(vec![
                vec![bare_call("a", "big_only"), ChatEvent::Done],
                vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
            ]);
            (Agent::new(provider.clone(), model, None, tools), provider)
        };
        let (agent, provider) = build("big");
        let mut history = user("go");
        run_with(agent, &mut history).await;
        assert_eq!(tool_answers(&history), ["finished"]);
        assert!(
            provider.seen.lock().unwrap()[0]
                .tools
                .iter()
                .any(|t| t.name == "big_only")
        );

        let (agent, provider) = build("small");
        let mut history = user("go");
        run_with(agent, &mut history).await;
        assert!(
            !provider.seen.lock().unwrap()[0]
                .tools
                .iter()
                .any(|t| t.name == "big_only"),
            "not offered"
        );
        assert_eq!(
            tool_answers(&history),
            ["error: unknown tool `big_only`"],
            "and not callable either"
        );
    }

    #[tokio::test]
    async fn a_prompt_section_can_be_added_in_order_replaced_by_name_and_taken_back_out() {
        let provider = Scripted::new(
            vec![vec![ChatEvent::Text("a".into()), ChatEvent::Done]; 3]
                .into_iter()
                .collect(),
        );
        let agent = Agent::new(
            provider.clone(),
            "m",
            Some("DIRECTIVE".to_owned()),
            toolbox(),
        );
        agent.set_section("rules", crate::sections::order::DEFAULT, "RULES");
        agent.set_section("early", 50, "EARLY");
        agent.set_dynamic_section("clock", 700, || Some("NOW".to_owned()));
        let (tx, _rx) = unbounded_channel();
        agent
            .run(&mut user("go"), &tx, Thinking::default())
            .await
            .expect("runs");
        let sent = provider.seen.lock().unwrap()[0]
            .system
            .clone()
            .unwrap_or_default();
        assert_eq!(sent, "EARLY\n\nDIRECTIVE\n\nRULES\n\nNOW");

        agent.set_section("system", crate::sections::order::SYSTEM, "REPLACED");
        assert!(agent.remove_section("early") && agent.remove_section("clock"));
        agent
            .run(&mut user("again"), &tx, Thinking::default())
            .await
            .expect("runs");
        assert_eq!(
            provider.seen.lock().unwrap()[1].system.as_deref(),
            Some("REPLACED\n\nRULES")
        );
        assert!(agent.remove_section("system"));
        agent
            .run(&mut user("third"), &tx, Thinking::default())
            .await
            .expect("runs");
        assert_eq!(
            provider.seen.lock().unwrap()[2].system.as_deref(),
            Some("DIRECTIVE\n\nRULES"),
            "the built-in comes back"
        );
    }

    #[tokio::test]
    async fn a_message_sent_during_a_run_is_added_before_the_next_model_call() {
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        let provider = Scripted::new(vec![
            vec![call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Text("done".into()), ChatEvent::Done],
        ]);
        let agent = Agent::new(provider.clone(), "m", None, tools);
        agent.steer("also mind the tests");
        assert!(agent.has_steering());
        let mut history = user("go");
        run_with(agent, &mut history).await;
        let typed: Vec<&str> = history
            .iter()
            .filter(|m| m.is_typed())
            .map(|m| m.text.as_str())
            .collect();
        assert_eq!(typed, ["go", "also mind the tests"]);
        assert!(
            history
                .iter()
                .filter(|m| m.is_typed())
                .all(|m| m.at.is_some() || m.text == "go")
        );
    }

    #[tokio::test]
    async fn a_message_sent_while_a_tool_runs_is_in_front_of_the_models_next_call() {
        let cell: Arc<std::sync::OnceLock<Arc<Agent>>> = Arc::new(std::sync::OnceLock::new());
        struct Steering(Arc<std::sync::OnceLock<Arc<Agent>>>);
        #[async_trait]
        impl Tool for Steering {
            fn name(&self) -> &str {
                "late"
            }
            async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
                if let Some(agent) = self.0.get() {
                    agent.steer("one more thing");
                }
                ToolResult {
                    content: "ok".into(),
                    is_error: false,
                    details: None,
                }
            }
        }
        let provider = Scripted::new(vec![
            vec![bare_call("a", "late"), ChatEvent::Done],
            vec![ChatEvent::Text("done".into()), ChatEvent::Done],
        ]);
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Steering(cell.clone()));
        let agent = Arc::new(Agent::new(provider.clone(), "m", None, tools));
        let _ = cell.set(agent.clone());
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        assert_eq!(provider.calls(), 2);
        let seen = provider.seen.lock().unwrap();
        assert!(
            seen[1].messages.iter().any(|m| m.text == "one more thing"),
            "the second call already carried it"
        );
    }

    /// Steers its agent while it is answering, as a person typing during a reply would.
    struct Interrupting {
        agent: Arc<std::sync::OnceLock<Arc<Agent>>>,
        inner: Arc<Scripted>,
    }

    #[async_trait]
    impl Provider for Interrupting {
        fn id(&self) -> &str {
            "interrupting"
        }
        async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
            if self.inner.calls() == 0
                && let Some(agent) = self.agent.get()
            {
                agent.steer("wait, also this");
            }
            self.inner.stream(request).await
        }
    }

    #[tokio::test]
    async fn jobs_start_only_so_many_runs_in_a_row_until_the_person_speaks() {
        let replies = (0..=MAX_WAKES + 1)
            .map(|_| vec![ChatEvent::Text("ok".into()), ChatEvent::Done])
            .collect();
        let agent = Agent::new(Scripted::new(replies), "m", None, toolbox());
        let mut history = Vec::new();
        for round in 0..MAX_WAKES {
            agent.steer(format!("job {round} ended"));
            let said = agent.take_waiting().expect("under the cap");
            history.push(Message::new(Role::User, said));
            let (tx, _rx) = unbounded_channel();
            agent
                .run(&mut history, &tx, Thinking::default())
                .await
                .expect("runs");
        }
        agent.steer("one more ended");
        assert_eq!(agent.take_waiting(), None);
        assert!(agent.has_steering(), "it waits for the person");
        history.push(Message::new(Role::User, "go on"));
        let (tx, _rx) = unbounded_channel();
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        agent.steer("another ended");
        assert!(agent.take_waiting().is_some());
    }

    /// A coding agent that works until it is stopped.
    struct Waiting;

    #[async_trait]
    impl crate::orchestration::CodingAgents for Waiting {
        async fn run(
            &self,
            _thread: &str,
            _job: &crate::orchestration::CodingJob,
            _events: &tokio::sync::mpsc::UnboundedSender<vivianne_providers::acp::AcpEvent>,
            cancel: &CancellationToken,
        ) -> Result<vivianne_providers::acp::TurnEnd, String> {
            cancel.cancelled().await;
            Ok(vivianne_providers::acp::TurnEnd {
                outcome: vivianne_providers::acp::Outcome::Stopped,
                reply: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn a_stopped_turn_stops_its_subagents_and_a_finished_one_leaves_them() {
        let agent = Agent::new(Scripted::new(Vec::new()), "m", None, toolbox());
        agent.set_thread("stop-subagents");
        let caller = agent.caller().expect("a caller");
        let job = crate::orchestration::CodingJob {
            agent: "claude-acp".to_owned(),
            folder: std::env::temp_dir(),
            prompt: "work".to_owned(),
            read_only: false,
        };
        let handle = crate::orchestration::start_coding(Arc::new(Waiting), &caller, "agent-1".into(), "works".into(), job)
            .expect("started");
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        agent.run(&mut history, &tx, Thinking::default()).await.expect("runs");
        assert!(!handle.status().is_done(), "a turn that finished leaves it working");
        let cancel = CancellationToken::new();
        cancel.cancel();
        history.push(Message::new(Role::User, "again"));
        agent.run_until(&mut history, &tx, Thinking::default(), &cancel).await.expect("stops");
        assert!(matches!(handle.status(), crate::orchestration::Status::Cancelled(_)), "{:?}", handle.status());
    }

    #[tokio::test]
    async fn steering_that_arrives_as_the_model_finishes_gets_one_more_step() {
        let scripted = Scripted::new(vec![
            vec![ChatEvent::Text("first answer".into()), ChatEvent::Done],
            vec![
                ChatEvent::Text("answer to the extra".into()),
                ChatEvent::Done,
            ],
        ]);
        let cell: Arc<std::sync::OnceLock<Arc<Agent>>> = Arc::new(std::sync::OnceLock::new());
        let provider = Arc::new(Interrupting {
            agent: cell.clone(),
            inner: scripted.clone(),
        });
        let agent = Arc::new(Agent::new(provider, "m", None, toolbox()));
        let _ = cell.set(agent.clone());
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        assert_eq!(
            scripted.calls(),
            2,
            "a second call answered what was said during the first"
        );
        assert_eq!(
            history.last().map(|m| m.text.as_str()),
            Some("answer to the extra")
        );
        assert!(!agent.has_steering());
    }

    #[tokio::test]
    async fn the_conversation_is_saved_before_each_model_request_and_before_the_tools_of_a_step_run()
     {
        let saved: Arc<Mutex<Vec<(usize, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let provider = Scripted::new(vec![
            vec![call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Text("done".into()), ChatEvent::Done],
        ]);
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        let agent = Agent::new(provider, "m", None, tools);
        let log = saved.clone();
        agent.set_checkpoint(Some(Checkpoint::new(move |history: &[Message]| {
            log.lock().unwrap().push((
                history.len(),
                history
                    .last()
                    .map(|m| m.role)
                    .map_or("none", |r| {
                        if r == Role::Tool {
                            "tool"
                        } else if r == Role::Assistant {
                            "assistant"
                        } else {
                            "user"
                        }
                    })
                    .to_owned(),
            ));
            Ok(())
        })));
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        let saved = saved.lock().unwrap().clone();
        // Before request 1 (the person's message); before the tool ran (the model's call is in); before request 2 (the result is in).
        assert_eq!(
            saved,
            [
                (1, "user".to_owned()),
                (2, "assistant".to_owned()),
                (3, "tool".to_owned())
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_save_stops_the_run_before_the_model_is_asked_or_a_tool_runs() {
        let provider = Scripted::new(vec![vec![call("c1", "echo"), ChatEvent::Done]]);
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        let agent = Agent::new(provider.clone(), "m", None, tools);
        agent.set_checkpoint(Some(Checkpoint::new(|_: &[Message]| {
            Err("disk full".to_owned())
        })));
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        let error = agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap_err();
        assert!(
            matches!(&error, AgentError::Checkpoint(why) if why == "disk full"),
            "{error}"
        );
        assert_eq!(provider.calls(), 0, "the model was never asked");
        // A save that fails only after the model answered stops the tools from running.
        let provider = Scripted::new(vec![vec![call("c1", "echo"), ChatEvent::Done]]);
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        let agent = Agent::new(provider.clone(), "m", None, tools);
        let saves = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = saves.clone();
        agent.set_checkpoint(Some(Checkpoint::new(move |_: &[Message]| {
            if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(())
            } else {
                Err("gone".to_owned())
            }
        })));
        let mut history = user("go");
        assert!(
            agent
                .run(&mut history, &tx, Thinking::default())
                .await
                .is_err()
        );
        assert_eq!(provider.calls(), 1);
        assert!(
            history.iter().all(|m| m.role != Role::Tool),
            "no tool ran, so there is no result: {history:?}"
        );
    }

    #[tokio::test]
    async fn without_a_place_to_save_nothing_is_asked_of_anyone() {
        let provider = Scripted::new(vec![vec![ChatEvent::Text("hi".into()), ChatEvent::Done]]);
        let agent = Agent::new(provider, "m", None, toolbox());
        let (tx, _rx) = unbounded_channel();
        let mut history = user("go");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
    }

    #[tokio::test]
    async fn a_resumed_conversation_is_back_in_the_directory_it_had_moved_to_and_the_model_is_told()
    {
        let root = std::fs::canonicalize(std::env::temp_dir())
            .expect("tmp")
            .join(format!("viv-resume-cwd-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(root.join("sub")).expect("dirs");
        let toolbox = Toolbox::coding(&root);
        let provider = Scripted::new(vec![vec![ChatEvent::Text("ok".into()), ChatEvent::Done]]);
        let agent = Agent::new(provider.clone(), "m", Some("DIRECTIVE".to_owned()), toolbox);
        let mut history = vec![
            Message::new(Role::User, "go into sub"),
            Message::assistant(
                String::new(),
                vec![ToolCall {
                    id: "w".into(),
                    name: "working_directory".into(),
                    args: json!({ "path": "sub" }),
                }],
            ),
            Message::tool_result("w", "Working directory: x".to_owned()),
            Message::assistant("done".to_owned(), Vec::new()),
            Message::new(Role::User, "now what"),
        ];
        let (tx, _rx) = unbounded_channel();
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        assert!(agent.tools.working_directory().moved());
        let system = provider.seen.lock().unwrap()[0]
            .system
            .clone()
            .unwrap_or_default();
        assert!(
            system.contains("Current working directory:") && system.contains("sub"),
            "{system}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn a_run_that_edits_a_file_with_a_tool_is_recorded_with_its_before_and_after() {
        let dir = std::env::temp_dir().join(format!(
            "viv-agent-changes-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(
            dir.join("notes.txt"),
            "one
two
",
        )
        .expect("seed");
        let write = ChatEvent::ToolCall(ToolCall {
            id: "w".into(),
            name: "write".into(),
            args: json!({ "path": "notes.txt", "content": "one
two
three
" }),
        });
        let provider = Scripted::new(vec![
            vec![write, ChatEvent::Done],
            vec![ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let mut tools = Toolbox::new(&dir);
        tools.add(Writer(dir.clone()));
        let recorder = Arc::new(
            crate::changes::Recorder::new(&dir, crate::changes::Limits::default())
                .expect("recorder"),
        );
        let agent = Agent::new(provider, "m", None, tools).with_changes(recorder.clone());
        let (tx, _rx) = unbounded_channel();
        let mut history = user("add a line");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .expect("runs");
        let summary = recorder.latest().await.expect("the run changed a file");
        assert_eq!(
            (summary.turn, summary.total, summary.added, summary.deleted),
            (1, 1, 1, 0)
        );
        assert_eq!(summary.files[0].display, "notes.txt");
        let crate::changes::FileDiff::Text { hunks, .. } =
            recorder.diff(1, 0).await.expect("reads").expect("a diff")
        else {
            panic!("text")
        };
        assert!(hunks[0].lines.contains(&"+three".to_owned()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limit_is_waited_out_and_leaves_no_trace_in_the_conversation() {
        let provider = Scripted::named(
            "p",
            vec![
                Reply::Refuse(RATE_LIMIT),
                Reply::FailFirst(RATE_LIMIT),
                Reply::Events(vec![ChatEvent::Text("hi".into()), ChatEvent::Done]),
            ],
        );
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        let retry = RetryPolicy {
            backoff: crate::resilience::Backoff {
                jitter: 0.0,
                ..Default::default()
            },
            ..Default::default()
        };
        agent(provider.clone())
            .with_retry(retry)
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        drop(tx);

        assert_eq!(provider.calls(), 3);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].text, "hi");
        let seen = drain(rx).await;
        let retries: Vec<_> = seen
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Retrying { attempt, delay, .. } => Some((*attempt, *delay)),
                _ => None,
            })
            .collect();
        assert_eq!(retries, [(1, ONE_SECOND), (2, 2 * ONE_SECOND)]);
        let texts = seen
            .iter()
            .filter(|e| matches!(e, AgentEvent::Text(_)))
            .count();
        assert_eq!(texts, 1, "the retried calls showed the user nothing");
    }

    #[tokio::test(start_paused = true)]
    async fn when_one_model_keeps_failing_the_next_is_used_and_the_switch_is_announced() {
        let broken = Scripted::named("a", (0..4).map(|_| Reply::Refuse(RATE_LIMIT)).collect());
        let working = Scripted::named(
            "b",
            vec![Reply::Events(vec![
                ChatEvent::Text("from b".into()),
                ChatEvent::Done,
            ])],
        );
        let agent = Agent::new(broken.clone(), "m1", None, toolbox())
            .with_fallbacks(vec![Target::new(working.clone(), "m2")]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        drop(tx);

        assert_eq!(history[1].text, "from b");
        let switch = drain(rx).await.into_iter().find_map(|e| match e {
            AgentEvent::Fallback { from, to, .. } => Some((from, to)),
            _ => None,
        });
        assert_eq!(switch, Some(("a/m1".to_owned(), "b/m2".to_owned())));

        // The rescuer is tried first next time, so the limited model is not asked again.
        let before = broken.calls();
        let (tx, _rx) = unbounded_channel();
        let mut history = user("again");
        agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap();
        assert_eq!(broken.calls(), before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_request_does_not_go_round_every_model() {
        let first = Scripted::named("a", vec![Reply::Refuse("400 invalid request: bad schema")]);
        let second = Scripted::named("b", vec![]);
        let agent = Agent::new(first, "m1", None, toolbox())
            .with_fallbacks(vec![Target::new(second.clone(), "m2")]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        let error = agent
            .run(&mut history, &tx, Thinking::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("bad schema"));
        assert_eq!(second.calls(), 0);
    }

    #[tokio::test]
    async fn stopping_mid_tool_still_answers_every_tool_call() {
        let provider = Scripted::new(vec![vec![
            call("c1", "hang"),
            call("c2", "echo"),
            ChatEvent::Done,
        ]]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stopper.cancel();
        });
        agent(provider)
            .run_until(&mut history, &tx, Thinking::default(), &cancel)
            .await
            .unwrap();
        drop(tx);

        let answered: Vec<_> = history
            .iter()
            .filter(|m| m.role == Role::Tool)
            .map(|m| m.tool_call_id.clone().unwrap_or_default())
            .collect();
        assert_eq!(answered, ["c1", "c2"]);
        assert!(history[2].text.contains("cancelled"));
        assert_eq!(drain(rx).await.last(), Some(&AgentEvent::Stopped));
    }
}
