//! The turn loop.

use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc::UnboundedSender;
use vivianne_core::{Message, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, Provider, ProviderError, ToolSpec};
use vivianne_tools::{ToolResult, Toolbox};

use crate::AgentEvent;

/// Same ceiling the old app used for chats.
const DEFAULT_MAX_STEPS: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    model: String,
    system: Option<String>,
    tools: Toolbox,
    max_steps: usize,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("provider", &self.provider.id())
            .field("model", &self.model)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl Agent {
    #[must_use]
    pub fn new(
        provider: Arc<dyn Provider>,
        model: &str,
        system: Option<String>,
        tools: Toolbox,
    ) -> Self {
        Self {
            provider,
            model: model.to_owned(),
            system,
            tools,
            max_steps: DEFAULT_MAX_STEPS,
        }
    }

    #[must_use]
    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// Run until the model stops asking for tools. New messages are appended to
    /// `history`. Dropping the returned future cancels the run.
    pub async fn run(
        &self,
        history: &mut Vec<Message>,
        events: &UnboundedSender<AgentEvent>,
    ) -> Result<(), AgentError> {
        for _ in 0..self.max_steps {
            let (text, calls) = self.stream_step(history, events).await?;
            let finished = calls.is_empty();
            history.push(Message::assistant(text, calls.clone()));
            if finished {
                send(events, AgentEvent::Done);
                return Ok(());
            }
            for call in calls {
                history.push(self.run_tool(&call, events).await);
            }
        }
        send(events, AgentEvent::StepLimit);
        Ok(())
    }

    async fn stream_step(
        &self,
        history: &[Message],
        events: &UnboundedSender<AgentEvent>,
    ) -> Result<(String, Vec<ToolCall>), AgentError> {
        let mut stream = self.provider.stream(&self.request(history)).await?;
        let (mut text, mut calls) = (String::new(), Vec::new());
        while let Some(event) = stream.next().await {
            match event? {
                ChatEvent::Text(piece) => {
                    text.push_str(&piece);
                    send(events, AgentEvent::Text(piece));
                }
                ChatEvent::ToolCall(call) => calls.push(call),
                ChatEvent::Done => break,
            }
        }
        Ok((text, calls))
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
        let result = self
            .tools
            .execute(&call.name, &call.args)
            .await
            .unwrap_or_else(|| ToolResult {
                content: format!("unknown tool `{}`", call.name),
                is_error: true,
                details: None,
            });
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
            },
        );
        Message::tool_result(&call.id, output)
    }

    fn request(&self, history: &[Message]) -> ChatRequest {
        ChatRequest {
            model: self.model.clone(),
            system: self.system.clone(),
            messages: history.to_vec(),
            tools: self
                .tools
                .definitions()
                .into_iter()
                .map(|d| ToolSpec {
                    name: d.name,
                    description: d.description,
                    parameters: d.parameters,
                })
                .collect(),
        }
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

    use async_trait::async_trait;
    use serde_json::{Value, json};
    use tokio::sync::mpsc::unbounded_channel;
    use vivianne_core::Role;
    use vivianne_providers::EventStream;
    use vivianne_tools::Tool;

    use super::*;

    /// Replays one scripted reply per call and records the requests it saw.
    struct Scripted {
        replies: Mutex<VecDeque<Vec<ChatEvent>>>,
        seen: Mutex<Vec<ChatRequest>>,
    }

    impl Scripted {
        fn new(replies: Vec<Vec<ChatEvent>>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into()),
                seen: Mutex::default(),
            })
        }
    }

    #[async_trait]
    impl Provider for Scripted {
        fn id(&self) -> &'static str {
            "scripted"
        }
        async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
            self.seen.lock().unwrap().push(request.clone());
            let reply = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![ChatEvent::Done]);
            Ok(Box::pin(futures::stream::iter(reply.into_iter().map(Ok))))
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

    fn echo_call(id: &str, tool: &str) -> ChatEvent {
        ChatEvent::ToolCall(ToolCall {
            id: id.into(),
            name: tool.into(),
            args: json!({"say": "pong"}),
        })
    }

    fn agent(provider: Arc<Scripted>) -> Agent {
        let mut tools = Toolbox::new(std::path::Path::new("."));
        tools.add(Echo);
        Agent::new(provider, "m", None, tools)
    }

    #[tokio::test]
    async fn tool_result_is_fed_back_and_the_loop_ends_on_plain_text() {
        let provider = Scripted::new(vec![
            vec![echo_call("c1", "echo"), ChatEvent::Done],
            vec![ChatEvent::Text("all done".into()), ChatEvent::Done],
        ]);
        let (tx, mut rx) = unbounded_channel();
        let mut history = vec![Message::new(Role::User, "ping")];

        agent(provider.clone())
            .run(&mut history, &tx)
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

        drop(tx);
        let mut seen = Vec::new();
        while let Some(e) = rx.recv().await {
            seen.push(e);
        }
        assert!(matches!(seen.first(), Some(AgentEvent::ToolStart { .. })));
        assert_eq!(seen.last(), Some(&AgentEvent::Done));
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_to_the_model_not_fatal() {
        let provider = Scripted::new(vec![vec![echo_call("c1", "nope")], vec![ChatEvent::Done]]);
        let (tx, _rx) = unbounded_channel();
        let mut history = vec![Message::new(Role::User, "x")];
        agent(provider).run(&mut history, &tx).await.unwrap();
        assert!(history[2].text.starts_with("error: unknown tool"));
    }

    #[tokio::test]
    async fn step_ceiling_stops_a_model_that_never_finishes() {
        let looping = (0..5).map(|_| vec![echo_call("c", "echo")]).collect();
        let (tx, mut rx) = unbounded_channel();
        let mut history = vec![Message::new(Role::User, "x")];
        agent(Scripted::new(looping))
            .with_max_steps(3)
            .run(&mut history, &tx)
            .await
            .unwrap();
        drop(tx);
        let mut last = None;
        while let Some(e) = rx.recv().await {
            last = Some(e);
        }
        assert_eq!(last, Some(AgentEvent::StepLimit));
    }
}
