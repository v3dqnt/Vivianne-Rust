//! The turn loop.

use std::sync::{Arc, Mutex, PoisonError};

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;
use vivianne_core::{Message, Mode, Thinking, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError, Spent, ToolSpec};
use vivianne_tools::{ToolResult, Toolbox};

use crate::context::{self, Breakdown};
use crate::AgentEvent;
use crate::resilience::{Chain, Failure, RetryPolicy, Stopped, retry};

/// Same ceiling the old app used for chats.
const DEFAULT_MAX_STEPS: usize = 100;

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
            mode: Mutex::new(Mode::default()),
        }
    }

    /// Changes what the agent is for: its instructions and the tools it may use.
    pub fn set_mode(&self, mode: Mode) {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner) = mode;
    }

    fn mode(&self) -> Mode {
        *self.mode.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The system prompt as sent: the standing instructions, then the mode's.
    fn system_prompt(&self, mode: Mode) -> Option<String> {
        match (&self.system, mode.prompt_section()) {
            (Some(system), Some(section)) => Some(format!("{system}\n\n{section}")),
            (Some(system), None) => Some(system.clone()),
            (None, section) => section.map(str::to_owned),
        }
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
        for _ in 0..self.max_steps {
            let streamed = tokio::select! {
                () = cancel.cancelled() => None,
                streamed = self.stream_step(history, events, thinking, cancel) => Some(streamed?),
            };
            let Some((text, calls, spent)) = streamed else {
                send(events, AgentEvent::Stopped);
                return Ok(());
            };
            if let Some(spent) = spent {
                send(events, usage_event(spent));
            }
            let finished = calls.is_empty();
            history.push(Message::assistant(text, calls.clone()));
            if finished {
                send(events, AgentEvent::Done);
                return Ok(());
            }
            let stopped = self.run_tools(&calls, history, events, cancel).await;
            if stopped {
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
            history.push(answer.unwrap_or_else(|| Message::tool_result(&call.id, CANCELLED_TOOL.to_owned())));
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

    async fn run_tool(&self, call: &ToolCall, events: &UnboundedSender<AgentEvent>) -> Message {
        send(events, AgentEvent::ToolStart { id: call.id.clone(), name: call.name.clone(), args: call.args.clone() });
        let mode = self.mode();
        // Not offering a tool is not enough: a model can still ask for one it remembers from earlier in the
        // conversation (or from before the mode changed), so the mode is enforced here too.
        let result = if mode.allows(&call.name) {
            self.tools.execute(&call.name, &call.args).await.unwrap_or_else(|| ToolResult {
                content: format!("unknown tool `{}`", call.name),
                is_error: true,
                details: None,
            })
        } else {
            ToolResult { content: format!("`{}` is not available in {} mode", call.name, mode.label()), is_error: true, details: None }
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
    async fn a_mode_adds_its_instruction_and_offers_only_the_tools_it_allows() {
        let provider = Scripted::new(vec![vec![ChatEvent::Done]]);
        let agent = Agent::new(provider.clone(), "m", Some("Be brief.".to_owned()), toolbox());
        agent.set_mode(Mode::Ask);
        let (tx, _rx) = unbounded_channel();
        agent.run(&mut user("x"), &tx, Thinking::default()).await.unwrap();
        let seen = provider.seen.lock().unwrap();
        assert!(seen[0].system.as_deref().is_some_and(|s| s.starts_with("Be brief.") && s.contains("Mode: Ask")));
        assert!(seen[0].tools.is_empty(), "the test tools (echo, hang) are not look-only tools");
    }

    #[tokio::test]
    async fn a_tool_the_mode_withholds_is_refused_even_if_the_model_asks_for_it() {
        let provider = Scripted::new(vec![vec![call("c1", "echo"), ChatEvent::Done], vec![ChatEvent::Done]]);
        let agent = Agent::new(provider, "m", None, toolbox());
        agent.set_mode(Mode::Explore);
        let (tx, _rx) = unbounded_channel();
        let mut history = user("x");
        agent.run(&mut history, &tx, Thinking::default()).await.unwrap();
        assert!(history[2].text.contains("not available in Explore mode"), "{}", history[2].text);
    }

    #[test]
    fn the_breakdown_follows_the_mode_because_withheld_tools_leave_the_window() {
        let agent = Agent::new(Scripted::new(vec![]), "m", Some("Be brief.".to_owned()), Toolbox::coding(std::path::Path::new(".")));
        let all = agent.breakdown(&[]);
        agent.set_mode(Mode::Ask);
        let ask = agent.breakdown(&[]);
        assert!(ask.tools > 0 && ask.tools < all.tools, "{all:?} {ask:?}");
        assert!(ask.system > all.system, "the mode's instruction is part of the system prompt");
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
