//! The turn loop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Mode, Role, Thinking, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError, Spent, ToolSpec};
use vivianne_tools::{ToolResult, Toolbox};

use crate::attempts::Attempts;
use crate::compact;
use crate::context::{self, Breakdown};
use crate::history;
use crate::AgentEvent;
use crate::resilience::{Chain, Failure, RetryPolicy, Stopped, retry};

/// Same ceiling the old app used for chats.
const DEFAULT_MAX_STEPS: usize = 100;

/// Longest the call that writes a checkpoint may take.
const SUMMARY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);

/// Longest the call that writes a title may take: the reply never waits for it, but a slow one is not worth keeping.
const TITLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// What a tool that was cut short tells the model, so every call it made has an answer.
const CANCELLED_TOOL: &str = "error: cancelled by the user before this finished";

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
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
        Self { provider, model: model.to_owned() }
    }

    /// `provider/model`, for telling the user.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}/{}", self.provider.id(), self.model)
    }
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target").field("provider", &self.provider.id()).field("model", &self.model).finish()
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
    /// Where to look for AGENTS.md and CLAUDE.md, and the name this conversation is told about changes under.
    instructions_from: Option<(Vec<String>, String)>,
    /// What those files said at the start of the current run.
    instructions: Mutex<Option<String>>,
    /// Asked for a block of standing context (core memory) at the start of every run.
    context_source: Option<ContextSource>,
    context: Mutex<Option<String>>,
}

/// Something that can say what the agent should always know, read at the start of every run.
#[derive(Clone)]
pub struct ContextSource(Arc<dyn Fn() -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync>);

impl ContextSource {
    pub fn new(source: impl Fn() -> futures::future::BoxFuture<'static, Option<String>> + Send + Sync + 'static) -> Self {
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
}

impl Agent {
    #[must_use]
    pub fn new(provider: Arc<dyn Provider>, model: &str, system: Option<String>, tools: Toolbox) -> Self {
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
            instructions_from: None,
            instructions: Mutex::new(None),
            context_source: None,
            context: Mutex::new(None),
        }
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
        let waiting = self.asking.lock().unwrap_or_else(PoisonError::into_inner).remove(id);
        waiting.is_some_and(|w| w.send(allow).is_ok())
    }

    /// Asks the user whether `call` may run, and waits. A dropped question (the run was stopped) counts as no.
    async fn approved(&self, call: &ToolCall, events: &UnboundedSender<AgentEvent>) -> bool {
        let (tx, rx) = oneshot::channel();
        self.asking.lock().unwrap_or_else(PoisonError::into_inner).insert(call.id.clone(), tx);
        let detail = ["command", "path"].iter().find_map(|k| call.args[k].as_str()).unwrap_or_default();
        send(events, AgentEvent::Approval { id: call.id.clone(), title: format!("{} {detail}", call.name).trim().to_owned() });
        rx.await.unwrap_or(false)
    }

    fn mode(&self) -> Mode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The system prompt as sent: the standing instructions, the project's, then the mode's.
    fn system_prompt(&self, mode: Mode) -> Option<String> {
        let project = self.instructions.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let standing = self.context.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let parts: Vec<&str> = [self.system.as_deref(), project.as_deref(), standing.as_deref(), mode.prompt_section()].into_iter().flatten().collect();
        (!parts.is_empty()).then(|| parts.join("\n\n"))
    }

    /// The tools the model is offered in `mode`.
    fn tool_specs(&self, mode: Mode) -> Vec<ToolSpec> {
        self.tools
            .definitions()
            .into_iter()
            .filter(|d| mode.allows(&d.name))
            .map(|d| ToolSpec { name: d.name, description: d.description, parameters: d.parameters })
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
        context::breakdown(self.system_prompt(mode).as_deref().unwrap_or_default(), &tools, history)
    }

    /// Reads the project's instruction files (AGENTS.md, CLAUDE.md, and the user's own in `~/.viv`) at the start of
    /// every run and adds them to the system prompt, so an edit lands on the next message.
    #[must_use]
    pub fn with_instructions(mut self, folders: Vec<String>) -> Self {
        self.instructions_from = Some((folders, uuid::Uuid::new_v4().to_string()));
        self
    }

    /// Adds a block of standing context (such as core memory) to the system prompt, asked for at the start of every
    /// run so a change lands on the next message.
    #[must_use]
    pub fn with_context(mut self, source: ContextSource) -> Self {
        self.context_source = Some(source);
        self
    }

    #[must_use]
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
        self.run_until(history, events, thinking, &CancellationToken::new()).await
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
        // The connected MCP servers' tools, read again if a server changed its list.
        self.tools.refresh().await;
        if let Some((folders, key)) = &self.instructions_from {
            let loaded = crate::instructions::load_for(Some(key), folders).await;
            *self.instructions.lock().unwrap_or_else(PoisonError::into_inner) = loaded;
        }
        if let Some(source) = &self.context_source {
            let standing = (source.0)().await;
            *self.context.lock().unwrap_or_else(PoisonError::into_inner) = standing;
        }
        let mut attempts = Attempts::default();
        let mut condensed_for_overflow = false;
        for _ in 0..self.max_steps {
            // A run that was cut off earlier may have left calls without results, which a provider refuses.
            history::repair(history);
            let made_room = tokio::select! {
                () = cancel.cancelled() => None,
                made = self.make_room(history, events, false) => Some(made),
            };
            if made_room.is_none() {
                send(events, AgentEvent::Stopped);
                return Ok(());
            }
            history::mask_old_results(history);
            let streamed = tokio::select! {
                () = cancel.cancelled() => None,
                streamed = self.stream_step(history, events, thinking, cancel) => Some(streamed),
            };
            let Some(streamed) = streamed else {
                send(events, AgentEvent::Stopped);
                return Ok(());
            };
            let (text, calls, spent) = match streamed {
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
            history.push(Message::assistant(text, calls.clone()));
            if finished {
                send(events, AgentEvent::Done);
                return Ok(());
            }
            let stopped = self.run_tools(&calls, history, events, cancel, &mut attempts).await;
            if stopped {
                // Nobody will answer what was still being asked.
                self.asking.lock().unwrap_or_else(PoisonError::into_inner).clear();
                send(events, AgentEvent::Stopped);
                return Ok(());
            }
        }
        send(events, AgentEvent::StepLimit);
        Ok(())
    }

    /// Runs the calls in order, adding each answer to `history`. `true` if the run was stopped, in
    /// which case the calls that did not finish are answered as cancelled.
    async fn run_tools(
        &self,
        calls: &[ToolCall],
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
        attempts: &mut Attempts,
    ) -> bool {
        let mut stopped = false;
        for call in calls {
            let answer = if stopped {
                None
            } else {
                tokio::select! {
                    () = cancel.cancelled() => None,
                    answer = self.run_tool(call, events) => Some(answer),
                }
            };
            stopped |= answer.is_none();
            let mut answer = answer.unwrap_or_else(|| Message::tool_result(&call.id, CANCELLED_TOOL.to_owned()));
            if !stopped {
                if let Some(error) = answer.text.strip_prefix("error: ") {
                    if let Some(note) = attempts.failed(call, error) {
                        answer.text.push_str(&note);
                    }
                } else {
                    attempts.succeeded(call);
                }
            }
            history.push(answer);
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
                send(events, AgentEvent::Fallback { from, to: target.name(), reason: Failure::from(previous).short() });
            }
            match self.open_with_retry(target, history, thinking, events, cancel).await {
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
        Err(last_error.unwrap_or_else(|| ProviderError::Unavailable("No model is set up to answer.".to_owned())).into())
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
        let request = self.request(target, history, thinking);
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
            open(target, &request).await.map_err(|error| Failure::from(&error))
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
    async fn make_room(&self, history: &mut Vec<Message>, events: &UnboundedSender<AgentEvent>, force: bool) -> bool {
        let window = self.window.load(Ordering::Relaxed);
        let total = compact::tokens_of(history);
        if !force && (window == 0 || (total as f32) < window as f32 * compact::THRESHOLD) {
            return false;
        }
        let basis = if window == 0 { total } else { window };
        let cut = history::pick_cut(history, (basis as f32 * compact::KEEP) as u32, compact::MIN_MESSAGES);
        if cut == 0 {
            return false;
        }
        let Some(summary) = self.summarise(&history[..cut], events).await else { return false };
        if !compact::shrinks(&summary, compact::tokens_of(&history[..cut])) {
            return false;
        }
        compact::replace_head(history, cut, &summary);
        send(events, AgentEvent::Compacted { before: total, after: compact::tokens_of(history) });
        true
    }

    /// Asks the first model for a checkpoint of `part`: once with more of each tool result, once with less (a
    /// call that failed on size gets a second go). `None` if neither gives one.
    async fn summarise(&self, part: &[Message], events: &UnboundedSender<AgentEvent>) -> Option<String> {
        let target = &self.targets[self.chain.order()[0]];
        for preview in compact::PREVIEW_CHARS {
            let request = ChatRequest {
                model: target.model.clone(),
                system: Some(compact::INSTRUCTION.to_owned()),
                messages: vec![Message::new(Role::User, compact::render_transcript(part, preview))],
                tools: Vec::new(),
                thinking: Thinking::Off,
            };
            // What the summariser says is not part of the conversation, so its text goes nowhere.
            let (sink, _unread) = tokio::sync::mpsc::unbounded_channel();
            let asked = async { collect(open(target, &request).await?, &sink).await };
            if let Ok(Ok((text, _, spent))) = tokio::time::timeout(SUMMARY_TIMEOUT, asked).await {
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
        };
        let (sink, _unread) = tokio::sync::mpsc::unbounded_channel();
        let asked = async { collect(open(target, &request).await?, &sink).await };
        let (text, _, _) = tokio::time::timeout(TITLE_TIMEOUT, asked).await.ok()?.ok()?;
        crate::title::parse_title(&text)
    }

    async fn run_tool(&self, call: &ToolCall, events: &UnboundedSender<AgentEvent>) -> Message {
        send(events, AgentEvent::ToolStart { id: call.id.clone(), name: call.name.clone(), args: call.args.clone() });
        let mode = self.mode();
        // Not offering a tool is not enough: a model can still ask for one it remembers from earlier in the
        // conversation (or from before the mode changed), so the mode is enforced here too.
        let result = if !mode.allows(&call.name) {
            ToolResult { content: format!("`{}` is not available in {} mode", call.name, mode.label()), is_error: true, details: None }
        } else if mode.needs_approval(&call.name) && !self.approved(call, events).await {
            ToolResult { content: "the user did not allow this".to_owned(), is_error: true, details: None }
        } else {
            self.tools.execute(&call.name, &call.args).await.unwrap_or_else(|| ToolResult {
                content: format!("unknown tool `{}`", call.name),
                is_error: true,
                details: None,
            })
        };
        let output = if result.is_error { format!("error: {}", result.content) } else { result.content };
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

    fn request(&self, target: &Target, history: &[Message], thinking: Thinking) -> ChatRequest {
        let mode = self.mode();
        ChatRequest { model: target.model.clone(), system: self.system_prompt(mode), messages: history.to_vec(), tools: self.tool_specs(mode), thinking }
    }
}

/// Starts a stream and reads its first event, so a failure that comes back as the first item is
/// seen here and not after the caller has started showing output.
async fn open(target: &Target, request: &ChatRequest) -> Result<Opened, ProviderError> {
    let mut rest = target.provider.stream(request).await?;
    let first = rest.next().await.transpose()?;
    Ok(Opened { first, rest })
}

/// What one model call came to: its text, the tool calls it asked for, and what it used.
type Step = (String, Vec<ToolCall>, Option<Spent>);

/// Reads a stream to its end, sending text on as it arrives. What the call used is summed, so a stream that
/// reports it in pieces still counts as one call.
async fn collect(opened: Opened, events: &UnboundedSender<AgentEvent>) -> Result<Step, ProviderError> {
    let Opened { first, rest } = opened;
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
    Ok((text, calls, spent))
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
            Arc::new(Self { name, replies: Mutex::new(replies.into()), seen: Mutex::default() })
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
            let reply = self.replies.lock().unwrap().pop_front().unwrap_or(Reply::Events(vec![ChatEvent::Done]));
            match reply {
                Reply::Events(events) => Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok)))),
                Reply::Refuse(why) => Err(ProviderError::Failed(why.to_owned())),
                Reply::FailFirst(why) => {
                    Ok(Box::pin(futures::stream::iter([Err(ProviderError::Failed(why.to_owned()))])))
                }
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
            ToolResult { content: args["say"].as_str().unwrap_or_default().to_owned(), is_error: false, details: None }
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
        ChatEvent::ToolCall(ToolCall { id: id.into(), name: tool.into(), args: json!({"say": "pong"}) })
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

        agent(provider.clone()).run(&mut history, &tx, Thinking::Low).await.unwrap();

        let roles: Vec<Role> = history.iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(history[2].text, "pong");
        assert_eq!(history[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(provider.seen.lock().unwrap()[1].messages.len(), 3);
        assert!(provider.seen.lock().unwrap().iter().all(|r| r.thinking == Thinking::Low));

        drop(tx);
        let seen = drain(rx).await;
        assert!(matches!(seen.first(), Some(AgentEvent::ToolStart { .. })));
        assert_eq!(seen.last(), Some(&AgentEvent::Done));
    }

    fn spent(input: u64, cost: Option<f64>) -> Spent {
        Spent { provider: "p".into(), model: "m".into(), input, output: 10, cache_read: 0, cache_write: 0, cost }
    }

    #[tokio::test]
    async fn each_model_call_reports_what_it_used_once_and_pieces_of_one_call_are_summed() {
        let provider = Scripted::new(vec![
            vec![ChatEvent::Usage(spent(100, Some(0.01))), call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Usage(spent(40, Some(0.01))), ChatEvent::Usage(spent(60, None)), ChatEvent::Text("ok".into()), ChatEvent::Done],
        ]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("ping");
        agent(provider).run(&mut history, &tx, Thinking::Low).await.unwrap();
        drop(tx);

        let used: Vec<_> = drain(rx).await.into_iter().filter_map(|e| match e {
            AgentEvent::Usage { input, output, cost, .. } => Some((input, output, cost)),
            _ => None,
        }).collect();
        assert_eq!(used, [(100, 10, Some(0.01)), (100, 20, None)], "the second call's cost is unknown once one piece is");
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
        let agent = Agent::new(provider.clone(), "m", Some("Be brief.".to_owned()), toolbox());
        agent.set_mode(Mode::Plan);
        let (tx, _rx) = unbounded_channel();
        agent.run(&mut user("x"), &tx, Thinking::default()).await.unwrap();
        let seen = provider.seen.lock().unwrap();
        assert!(seen[0].system.as_deref().is_some_and(|s| s.starts_with("Be brief.") && s.contains("PLAN MODE")));
        assert!(seen[0].tools.is_empty(), "the test tools (echo, hang) are not look-only tools");
    }

    #[tokio::test]
    async fn plan_mode_refuses_a_tool_even_if_the_model_asks_for_it() {
        let provider = Scripted::new(vec![vec![call("c1", "echo"), ChatEvent::Done], vec![ChatEvent::Done]]);
        let agent = Agent::new(provider, "m", None, toolbox());
        agent.set_mode(Mode::Plan);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
        assert!(history[2].text.contains("not available in Plan mode"), "{}", history[2].text);
    }

    /// Ask mode stops at a tool that changes things until the user answers; no answer, or no, means it never runs.
    #[tokio::test]
    async fn ask_mode_waits_for_a_yes_before_a_tool_runs() {
        for (allow, ran) in [(true, true), (false, false)] {
            let provider = Scripted::new(vec![vec![call("c1", "echo"), ChatEvent::Done], vec![ChatEvent::Done]]);
            let agent = Arc::new(Agent::new(provider, "m", None, toolbox()));
            agent.set_mode(Mode::Ask);
            let (tx, mut rx) = unbounded_channel();
            let task = tokio::spawn({
                let agent = agent.clone();
                async move {
                    let mut history = user("x");
                    agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
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
            assert_eq!(history[2].text.contains("did not allow"), !ran, "{}", history[2].text);
        }
    }

    /// A conversation of `n` messages of about 100 tokens each, ending on the user's turn.
    fn long_history(n: usize) -> Vec<Message> {
        (0..n).map(|i| Message::new(if i % 2 == 0 { Role::User } else { Role::Assistant }, "x".repeat(400))).collect()
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
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
        drop(tx);
        let seen = provider.seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "one call to summarise, one to answer");
        assert_eq!(seen[0].system.as_deref(), Some(compact::INSTRUCTION));
        assert!(seen[1].messages[0].text.starts_with(compact::OPEN) && seen[1].messages[0].text.contains("SUMMARY"));
        assert!(seen[1].messages.len() < 9, "the older messages are gone from what is sent");
        assert!(matches!(history.last(), Some(m) if m.text == "answer"));
        assert!(futures::executor::block_on(drain(rx)).iter().any(|e| matches!(e, AgentEvent::Compacted { .. })));
    }

    #[tokio::test]
    async fn a_request_the_provider_says_is_too_large_is_condensed_and_asked_again_once() {
        let provider = Scripted::named(
            "scripted",
            vec![Reply::Refuse("400: maximum context length is 8192 tokens"), Reply::Events(said("SUMMARY")), Reply::Events(said("answer"))],
        );
        let agent = Agent::new(provider.clone(), "m", None, toolbox());
        let (tx, _rx) = unbounded_channel();
        let mut history = long_history(9);
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
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
            Message::assistant("", vec![ToolCall { id: "c".into(), name: "echo".into(), args: json!({}) }]),
            Message::new(Role::User, "hello?"),
        ];
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
        let sent = &provider.seen.lock().unwrap()[0].messages;
        assert_eq!(sent[2].tool_call_id.as_deref(), Some("c"), "the call has an answer right after it");
    }

    #[tokio::test]
    async fn standing_context_is_asked_for_at_the_start_of_a_run_and_joins_the_system_prompt() {
        let provider = Scripted::new(vec![vec![ChatEvent::Done]]);
        let agent = Agent::new(provider.clone(), "m", Some("Be brief.".to_owned()), toolbox())
            .with_context(ContextSource::new(|| Box::pin(async { Some("<core_memory>prefers tabs</core_memory>".to_owned()) })));
        let (tx, _rx) = unbounded_channel();
        agent.run(&mut user("x"), &tx, Thinking::default()).await.unwrap();
        let system = provider.seen.lock().unwrap()[0].system.clone().unwrap();
        assert!(system.starts_with("Be brief.") && system.contains("prefers tabs"));
    }

    #[tokio::test]
    async fn a_title_comes_from_the_model_and_is_none_when_it_does_not_answer_with_one() {
        let agent = Agent::new(Scripted::new(vec![said("\"Fix login timeout\".")]), "m", None, toolbox());
        assert_eq!(agent.title("my login times out").await.as_deref(), Some("Fix login timeout"));
        let agent = Agent::new(Scripted::new(vec![said(&"word ".repeat(30))]), "m", None, toolbox());
        assert_eq!(agent.title("x").await, None);
    }

    #[test]
    fn calls_with_no_id_or_a_repeated_id_get_their_own() {
        let call = |id: &str| ToolCall { id: id.into(), name: "read".into(), args: json!({}) };
        let ids: Vec<_> = unique_ids(vec![call(""), call("a"), call("a"), call("")], 7).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["call_7_0", "a", "call_7_2", "call_7_3"]);
    }

    #[test]
    fn the_breakdown_follows_the_mode_because_withheld_tools_leave_the_window() {
        let agent = Agent::new(Scripted::new(vec![]), "m", Some("Be brief.".to_owned()), Toolbox::coding(std::path::Path::new(".")));
        let all = agent.breakdown(&[]);
        agent.set_mode(Mode::Plan);
        let plan = agent.breakdown(&[]);
        assert!(plan.tools > 0 && plan.tools < all.tools, "{all:?} {plan:?}");
        assert!(plan.system > all.system, "the mode's instruction is part of the system prompt");
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_to_the_model_not_fatal() {
        let provider = Scripted::new(vec![vec![call("c1", "nope")], vec![ChatEvent::Done]]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        agent(provider).run(&mut history, &tx, Thinking::default()).await.unwrap();
        assert!(history[2].text.starts_with("error: unknown tool"));
    }

    #[tokio::test]
    async fn step_ceiling_stops_a_model_that_never_finishes() {
        let looping = (0..5).map(|_| vec![call("c", "echo")]).collect();
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        agent(Scripted::new(looping)).with_max_steps(3).run(&mut history, &tx, Thinking::default()).await.unwrap();
        drop(tx);
        assert_eq!(drain(rx).await.last(), Some(&AgentEvent::StepLimit));
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
        let retry = RetryPolicy { backoff: crate::resilience::Backoff { jitter: 0.0, ..Default::default() }, ..Default::default() };
        agent(provider.clone()).with_retry(retry).run(&mut history, &tx, Thinking::default()).await.unwrap();
        drop(tx);

        assert_eq!(provider.calls(), 3);
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].text, "hi");
        let seen = drain(rx).await;
        let retries: Vec<_> = seen.iter().filter_map(|e| match e {
            AgentEvent::Retrying { attempt, delay, .. } => Some((*attempt, *delay)),
            _ => None,
        }).collect();
        assert_eq!(retries, [(1, ONE_SECOND), (2, 2 * ONE_SECOND)]);
        let texts = seen.iter().filter(|e| matches!(e, AgentEvent::Text(_))).count();
        assert_eq!(texts, 1, "the retried calls showed the user nothing");
    }

    #[tokio::test(start_paused = true)]
    async fn when_one_model_keeps_failing_the_next_is_used_and_the_switch_is_announced() {
        let broken = Scripted::named("a", (0..4).map(|_| Reply::Refuse(RATE_LIMIT)).collect());
        let working = Scripted::named("b", vec![Reply::Events(vec![ChatEvent::Text("from b".into()), ChatEvent::Done])]);
        let agent = Agent::new(broken.clone(), "m1", None, toolbox())
            .with_fallbacks(vec![Target::new(working.clone(), "m2")]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
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
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
        assert_eq!(broken.calls(), before);
    }

    #[tokio::test(start_paused = true)]
    async fn a_bad_request_does_not_go_round_every_model() {
        let first = Scripted::named("a", vec![Reply::Refuse("400 invalid request: bad schema")]);
        let second = Scripted::named("b", vec![]);
        let agent = Agent::new(first, "m1", None, toolbox()).with_fallbacks(vec![Target::new(second.clone(), "m2")]);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        let error = agent.run(&mut history, &tx, Thinking::default()).await.unwrap_err();
        assert!(error.to_string().contains("bad schema"));
        assert_eq!(second.calls(), 0);
    }

    #[tokio::test]
    async fn stopping_mid_tool_still_answers_every_tool_call() {
        let provider = Scripted::new(vec![vec![call("c1", "hang"), call("c2", "echo"), ChatEvent::Done]]);
        let (tx, rx) = unbounded_channel();
        let mut history = user("x");
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            stopper.cancel();
        });
        agent(provider).run_until(&mut history, &tx, Thinking::default(), &cancel).await.unwrap();
        drop(tx);

        let answered: Vec<_> = history.iter().filter(|m| m.role == Role::Tool).map(|m| m.tool_call_id.clone().unwrap_or_default()).collect();
        assert_eq!(answered, ["c1", "c2"]);
        assert!(history[2].text.contains("cancelled"));
        assert_eq!(drain(rx).await.last(), Some(&AgentEvent::Stopped));
    }
}
