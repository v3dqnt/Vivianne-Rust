//! Vivianne's old provider layer (`vivianne-ai`) as a [`Provider`].
//!
//! The old streaming functions report on an event bus, one `ai-stream-token` event per chunk and a
//! last one carrying the whole assistant message. This adapter starts the right function for the
//! provider's kind, listens to that stream's events, and turns them into [`ChatEvent`]s: text as it
//! arrives, then the tool calls found in the finished message.

use std::collections::HashMap;

use async_trait::async_trait;
use tokio::sync::broadcast::{Receiver, error::RecvError};
use vivianne_ai::ai::api::{anthropic, google, openai_completions, openai_responses};
use vivianne_ai::ai::provider_registry::{ApiKind, ProviderSpec, provider_spec, resolve_base_url};
use vivianne_ai::ai::types::{
    AssistantMessage, Context, Message as AiMessage, MessageContent, StreamTokenEvent,
    ToolCall as AiToolCall, Usage,
};
use vivianne_ai::events::{Event, Events};
use vivianne_core::{Message, Role, Thinking, ToolCall};

use crate::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError, Spent, ToolSpec};

const STREAM_TOKEN_EVENT: &str = "ai-stream-token";

/// The Vercel AI Gateway is not in the old registry; it speaks OpenAI chat completions.
pub const VERCEL_GATEWAY: ProviderSpec = ProviderSpec {
    default_base_url: Some("https://ai-gateway.vercel.sh/v1"),
    api_kind: ApiKind::OpenAiChat,
};

/// What the model may spend on one reply. Thinking comes out of this budget, so the bigger the
/// thinking level, the more room the reply needs.
const fn max_output_tokens(thinking: Thinking) -> u32 {
    match thinking {
        Thinking::Off | Thinking::Low => 8_192,
        Thinking::Medium => 16_384,
        Thinking::High => 32_768,
    }
}

/// Anthropic's thinking budget for a level, in tokens; below the reply limit, as the API demands.
const fn thinking_budget_tokens(thinking: Thinking) -> u32 {
    match thinking {
        Thinking::Off => 0,
        Thinking::Low => 2_048,
        Thinking::Medium => 8_192,
        Thinking::High => 16_384,
    }
}

/// One provider from the registry (`openai`, `anthropic`, `google`, `groq`...), with its key.
#[derive(Debug)]
pub struct AiProvider {
    provider: String,
    base_url: Option<String>,
    api_key: Option<String>,
}

impl AiProvider {
    /// `base_url` overrides the registry's default (gateways and local servers need it).
    #[must_use]
    pub fn new(provider: &str, base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            provider: provider.to_owned(),
            base_url,
            api_key,
        }
    }

    fn spec(&self) -> Result<ProviderSpec, ProviderError> {
        provider_spec(&self.provider)
            .or_else(|| (self.provider == "vercel").then_some(VERCEL_GATEWAY))
            .ok_or_else(|| {
                ProviderError::Unavailable(format!("unknown provider `{}`", self.provider))
            })
    }

    fn base_url_for(&self, spec: &ProviderSpec) -> Result<String, ProviderError> {
        resolve_base_url(spec, self.base_url.as_deref())
            .map(str::to_owned)
            .ok_or_else(|| {
                ProviderError::Unavailable(format!("`{}` needs a base URL", self.provider))
            })
    }
}

#[async_trait]
impl Provider for AiProvider {
    fn id(&self) -> &str {
        &self.provider
    }

    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let spec = self.spec()?;
        let call = StreamCall {
            kind: spec.api_kind,
            base_url: self.base_url_for(&spec)?,
            provider: self.provider.clone(),
            model: request.model.clone(),
            api_key: self.api_key.clone(),
            thinking: request.thinking,
            context: to_context(request),
        };
        let events = Events::new();
        let stream_id = new_stream_id();
        // Listen before the stream starts, or its first tokens are missed.
        let tokens = events.subscribe();
        tokio::spawn(call.run(events, stream_id.clone()));
        Ok(chat_events(tokens, stream_id))
    }
}

// ── Starting a stream ───────────────────────────────────────────────────────

/// Everything one call to a streaming function needs.
struct StreamCall {
    kind: ApiKind,
    base_url: String,
    provider: String,
    model: String,
    api_key: Option<String>,
    thinking: Thinking,
    context: Context,
}

impl StreamCall {
    async fn run(self, events: Events, stream_id: String) {
        match self.kind {
            ApiKind::AnthropicMessages => self.run_anthropic(events, stream_id).await,
            ApiKind::OpenAiChat => self.run_openai_chat(events, stream_id).await,
            ApiKind::OpenAiResponses => self.run_openai_responses(events, stream_id).await,
            ApiKind::GoogleGenerative => self.run_google(events, stream_id).await,
            other => self.refuse(
                &events,
                stream_id,
                &format!("{other:?} providers are not wired yet"),
            ),
        }
    }

    async fn run_anthropic(self, events: Events, stream_id: String) {
        let budget = thinking_budget_tokens(self.thinking);
        let options = anthropic::AnthropicOptions {
            api_key: self.api_key,
            max_tokens: Some(max_output_tokens(self.thinking)),
            thinking_enabled: Some(budget > 0),
            thinking_budget_tokens: Some(budget),
            ..Default::default()
        };
        anthropic::stream(
            events,
            stream_id,
            self.model,
            self.provider,
            Some(self.base_url),
            self.context,
            options,
        )
        .await;
    }

    async fn run_openai_chat(self, events: Events, stream_id: String) {
        let options = openai_completions::OpenAICompletionsOptions {
            api_key: self.api_key,
            max_tokens: Some(max_output_tokens(self.thinking)),
            reasoning_effort: self.thinking.effort().map(str::to_owned),
            ..Default::default()
        };
        openai_completions::stream_chat_completions(
            events,
            stream_id,
            &self.base_url,
            &self.model,
            &self.provider,
            self.context,
            options,
        )
        .await;
    }

    async fn run_openai_responses(self, events: Events, stream_id: String) {
        let options = openai_responses::OpenAIResponsesOptions {
            api_key: self.api_key,
            max_tokens: Some(max_output_tokens(self.thinking)),
            reasoning_effort: self.thinking.effort().map(str::to_owned),
            ..Default::default()
        };
        openai_responses::stream_responses(
            events,
            stream_id,
            &self.base_url,
            &self.model,
            &self.provider,
            self.context,
            options,
        )
        .await;
    }

    async fn run_google(self, events: Events, stream_id: String) {
        let options = google::GoogleOptions {
            api_key: self.api_key,
            max_tokens: Some(max_output_tokens(self.thinking)),
            ..Default::default()
        };
        let model = google::GoogleModel {
            id: self.model,
            provider: self.provider,
            api: String::new(),
            reasoning: false,
        };
        google::stream(
            events,
            stream_id,
            model,
            Some(self.base_url),
            self.context,
            options,
        )
        .await;
    }

    /// End the stream at once with an error the caller will read as the reply failing.
    fn refuse(&self, events: &Events, stream_id: String, reason: &str) {
        let message = AssistantMessage {
            role: "assistant".to_owned(),
            content: Vec::new(),
            api: String::new(),
            provider: self.provider.clone(),
            model: self.model.clone(),
            usage: Usage::default(),
            stop_reason: "error".to_owned(),
            error_message: Some(reason.to_owned()),
            timestamp: 0,
            diagnostics: None,
        };
        let token = StreamTokenEvent {
            stream_id,
            delta: String::new(),
            done: true,
            message: Some(message),
            is_thinking: false,
        };
        let _ = events.emit(STREAM_TOKEN_EVENT, token);
    }
}

/// Unique on this process, which is all one private bus needs.
fn new_stream_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!("stream-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

// ── Conversation to the old layer's shapes ──────────────────────────────────

fn to_context(request: &ChatRequest) -> Context {
    let tool_names = tool_names_by_call(&request.messages);
    Context {
        messages: request
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let mut message = to_ai_message(m, &tool_names);
                for image in request.images.iter().filter(|image| image.message == i) {
                    message.content_blocks.push(MessageContent::Text {
                        text: image.label.clone(),
                    });
                    message.content_blocks.push(MessageContent::Image {
                        url: None,
                        base64: Some(image.base64.clone()),
                        media_type: Some(image.media_type.clone()),
                    });
                }
                message
            })
            .collect(),
        system_prompt: request.system.clone(),
        tools: (!request.tools.is_empty())
            .then(|| request.tools.iter().map(tool_definition).collect()),
    }
}

/// `{name, description, parameters}`: the shape every old provider function reads tools in.
fn tool_definition(tool: &ToolSpec) -> serde_json::Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "parameters": tool.parameters,
    })
}

/// A tool result names its call by id only; the providers also want the tool's name.
fn tool_names_by_call(messages: &[Message]) -> HashMap<&str, &str> {
    messages
        .iter()
        .flat_map(|m| &m.tool_calls)
        .map(|call| (call.id.as_str(), call.name.as_str()))
        .collect()
}

fn to_ai_message(message: &Message, tool_names: &HashMap<&str, &str>) -> AiMessage {
    let text = Some(message.text.clone());
    match message.role {
        Role::Assistant => AiMessage {
            role: "assistant".to_owned(),
            content_blocks: assistant_blocks(message),
            text,
            ..AiMessage::default()
        },
        Role::Tool => {
            let call_id = message.tool_call_id.clone().unwrap_or_default();
            AiMessage {
                role: "toolResult".to_owned(),
                content_blocks: vec![MessageContent::Text {
                    text: message.text.clone(),
                }],
                tool_name: tool_names.get(call_id.as_str()).map(|n| (*n).to_owned()),
                tool_call_id: Some(call_id),
                text,
                ..AiMessage::default()
            }
        }
        Role::User | Role::System => AiMessage {
            role: "user".to_owned(),
            content_blocks: vec![MessageContent::Text {
                text: message.text.clone(),
            }],
            text,
            ..AiMessage::default()
        },
    }
}

fn assistant_blocks(message: &Message) -> Vec<MessageContent> {
    let text = (!message.text.is_empty()).then(|| MessageContent::Text {
        text: message.text.clone(),
    });
    let calls = message.tool_calls.iter().map(|call| {
        MessageContent::ToolCall(AiToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.args.clone(),
            thought_signature: None,
        })
    });
    text.into_iter().chain(calls).collect()
}

// ── The stream's events to ChatEvents ───────────────────────────────────────

/// Where a stream is in its life.
enum Phase {
    /// Reading this stream's events from the bus.
    Streaming {
        tokens: Receiver<Event>,
        stream_id: String,
    },
    /// The last events (final text, tool calls, `Done`) are being handed out.
    Finishing(std::vec::IntoIter<ChatEvent>),
}

type Step = Option<(Result<ChatEvent, ProviderError>, Option<Phase>)>;

fn chat_events(tokens: Receiver<Event>, stream_id: String) -> EventStream {
    let start = Some(Phase::Streaming { tokens, stream_id });
    Box::pin(futures::stream::unfold(start, |phase| async move {
        match phase? {
            Phase::Finishing(mut rest) => {
                let event = rest.next()?;
                Some((Ok(event), Some(Phase::Finishing(rest))))
            }
            Phase::Streaming { tokens, stream_id } => read_until_event(tokens, stream_id).await,
        }
    }))
}

/// Read the bus until this stream says something the agent should hear.
async fn read_until_event(mut tokens: Receiver<Event>, stream_id: String) -> Step {
    loop {
        let token = match tokens.recv().await {
            Ok((name, payload)) if name == STREAM_TOKEN_EVENT => {
                match serde_json::from_value::<StreamTokenEvent>(payload) {
                    Ok(token) if token.stream_id == stream_id => token,
                    _ => continue,
                }
            }
            Ok(_) | Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => return Some((Ok(ChatEvent::Done), None)),
        };
        if token.done {
            return Some(finish(token));
        }
        if !token.is_thinking && !token.delta.is_empty() {
            let next = Phase::Streaming { tokens, stream_id };
            return Some((Ok(ChatEvent::Text(token.delta)), Some(next)));
        }
    }
}

/// The stream's last token: a failure, or the closing events.
fn finish(token: StreamTokenEvent) -> (Result<ChatEvent, ProviderError>, Option<Phase>) {
    if let Some(reason) = token.message.as_ref().and_then(failure_of) {
        return (Err(ProviderError::Failed(reason)), None);
    }
    let mut events = Vec::new();
    if !token.delta.is_empty() {
        events.push(ChatEvent::Text(token.delta));
    }
    events.extend(
        token
            .message
            .as_ref()
            .map_or_else(|| vec![ChatEvent::Done], finishing_events),
    );
    let mut events = events.into_iter();
    let first = events.next().unwrap_or(ChatEvent::Done);
    (Ok(first), Some(Phase::Finishing(events)))
}

/// What a finished message used. `None` when it reports nothing, so a call that counted no tokens is not
/// shown as a spend.
fn spent_by(message: &AssistantMessage) -> Option<Spent> {
    let usage = &message.usage;
    if usage.input == 0 && usage.output == 0 && usage.total_tokens == 0 {
        return None;
    }
    let cost = usage.cost.as_ref().map(|c| c.total).or_else(|| {
        vivianne_ai::ai::pricing::compute_cost(&message.provider, &message.model, usage)
    });
    Some(Spent {
        provider: message.provider.clone(),
        model: message.model.clone(),
        input: u64::from(usage.input),
        output: u64::from(usage.output),
        cache_read: u64::from(usage.cache_read),
        cache_write: u64::from(usage.cache_write),
        cost,
    })
}

/// What a finished message used, the tool calls it holds, then `Done`.
fn finishing_events(message: &AssistantMessage) -> Vec<ChatEvent> {
    let usage = spent_by(message).map(ChatEvent::Usage);
    let calls = message.content.iter().filter_map(|block| match block {
        MessageContent::ToolCall(call) => Some(ChatEvent::ToolCall(ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            args: call.arguments.clone(),
        })),
        _ => None,
    });
    usage
        .into_iter()
        .chain(calls)
        .chain([ChatEvent::Done])
        .collect()
}

/// Why a finished message counts as a failure, if it does.
fn failure_of(message: &AssistantMessage) -> Option<String> {
    (message.stop_reason == "error").then(|| {
        message
            .error_message
            .clone()
            .unwrap_or_else(|| "the provider reported an error".to_owned())
    })
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use serde_json::json;

    use super::*;

    fn token(
        id: &str,
        delta: &str,
        done: bool,
        message: Option<AssistantMessage>,
    ) -> StreamTokenEvent {
        StreamTokenEvent {
            stream_id: id.into(),
            delta: delta.into(),
            done,
            message,
            is_thinking: false,
        }
    }

    fn message(
        stop_reason: &str,
        content: Vec<MessageContent>,
        error: Option<&str>,
    ) -> AssistantMessage {
        AssistantMessage {
            role: "assistant".into(),
            content,
            api: "x".into(),
            provider: "p".into(),
            model: "m".into(),
            usage: Usage::default(),
            stop_reason: stop_reason.into(),
            error_message: error.map(str::to_owned),
            timestamp: 0,
            diagnostics: None,
        }
    }

    async fn collect(
        events: &Events,
        tokens: Vec<StreamTokenEvent>,
    ) -> Vec<Result<ChatEvent, ProviderError>> {
        let stream = chat_events(events.subscribe(), "a".into());
        for t in tokens {
            events.emit(STREAM_TOKEN_EVENT, t).unwrap();
        }
        stream.collect().await
    }

    #[tokio::test]
    async fn text_streams_then_tool_calls_then_done_and_other_streams_are_ignored() {
        let call = MessageContent::ToolCall(AiToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: json!({"path": "a"}),
            thought_signature: None,
        });
        let done = message("toolUse", vec![call], None);
        let events = Events::new();
        let got = collect(
            &events,
            vec![
                token("b", "not mine", false, None),
                token("a", "Hel", false, None),
                StreamTokenEvent {
                    is_thinking: true,
                    ..token("a", "thinking", false, None)
                },
                token("a", "lo", false, None),
                token("a", "", true, Some(done)),
            ],
        )
        .await;
        let got: Vec<_> = got.into_iter().map(Result::unwrap).collect();
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            args: json!({"path": "a"}),
        };
        assert_eq!(
            got,
            vec![
                ChatEvent::Text("Hel".into()),
                ChatEvent::Text("lo".into()),
                ChatEvent::ToolCall(call),
                ChatEvent::Done
            ]
        );
    }

    #[tokio::test]
    async fn a_finished_call_reports_what_it_used_and_a_call_with_no_tokens_reports_nothing() {
        let mut counted = message("stop", vec![], None);
        counted.usage = Usage {
            input: 100,
            output: 20,
            cache_read: 5,
            cache_write: 1,
            total_tokens: 120,
            cost: None,
        };
        let events = Events::new();
        let got = collect(&events, vec![token("a", "hi", true, Some(counted))]).await;
        let got: Vec<_> = got.into_iter().map(Result::unwrap).collect();
        let [ChatEvent::Text(_), ChatEvent::Usage(spent), ChatEvent::Done] = got.as_slice() else {
            panic!("unexpected events: {got:?}")
        };
        assert_eq!(
            (
                spent.input,
                spent.output,
                spent.cache_read,
                spent.cache_write
            ),
            (100, 20, 5, 1)
        );
        assert_eq!((spent.provider.as_str(), spent.model.as_str()), ("p", "m"));
        assert_eq!(
            spent.cost, None,
            "a model with no known price is unknown, not free"
        );
    }

    #[tokio::test]
    async fn a_failed_message_becomes_an_error() {
        let events = Events::new();
        let failed = message("error", vec![], Some("401 unauthorized"));
        let got = collect(&events, vec![token("a", "", true, Some(failed))]).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_ref().unwrap_err().to_string(), "401 unauthorized");
    }

    #[test]
    fn a_conversation_with_tools_becomes_the_old_layers_context() {
        let call = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            args: json!({"path": "a"}),
        };
        let request = ChatRequest {
            model: "m".into(),
            system: Some("sys".into()),
            messages: vec![
                Message::new(Role::User, "hi"),
                Message::assistant("looking", vec![call]),
                Message::tool_result("c1", "contents"),
            ],
            tools: vec![ToolSpec {
                name: "read".into(),
                description: "reads".into(),
                parameters: json!({"type": "object"}),
            }],
            thinking: Thinking::Medium,
            images: Vec::new(),
        };
        let context = to_context(&request);
        assert_eq!(context.system_prompt.as_deref(), Some("sys"));
        assert_eq!(
            context.tools.unwrap()[0]["parameters"],
            json!({"type": "object"})
        );
        let roles: Vec<_> = context.messages.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, ["user", "assistant", "toolResult"]);
        assert!(matches!(
            context.messages[1].content_blocks[1],
            MessageContent::ToolCall(_)
        ));
        let result = &context.messages[2];
        assert_eq!(result.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(result.tool_name.as_deref(), Some("read"));
    }

    #[test]
    fn the_thinking_budget_always_fits_inside_the_reply_limit() {
        assert_eq!(thinking_budget_tokens(Thinking::Off), 0);
        for level in Thinking::ALL {
            assert!(thinking_budget_tokens(level) < max_output_tokens(level));
        }
    }

    #[test]
    fn the_vercel_gateway_and_unknown_providers_resolve() {
        let gateway = AiProvider::new("vercel", None, None);
        let url = gateway.base_url_for(&gateway.spec().unwrap()).unwrap();
        assert_eq!(url, "https://ai-gateway.vercel.sh/v1");
        let unknown = AiProvider::new("nope", None, None);
        assert!(
            unknown
                .spec()
                .err()
                .unwrap()
                .to_string()
                .contains("unknown provider")
        );
    }
}
