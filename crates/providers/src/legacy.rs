//! The old provider layer (`vivianne-ai`) behind the [`Provider`] trait.
//!
//! Bare minimum: plain text in, streamed text out. No tools, thinking, retries or
//! compaction yet. It starts the old streaming function for the provider's kind,
//! listens on the event bus for that stream's tokens and turns them into [`ChatEvent`]s.

use async_trait::async_trait;
use tokio::sync::broadcast::error::RecvError;
use vivianne_ai::ai::api::{anthropic, google, openai_completions, openai_responses};
use vivianne_ai::ai::provider_registry::{ApiKind, provider_spec, resolve_base_url, ProviderSpec};
use vivianne_ai::ai::types::{Context, Message as AiMessage, MessageContent, StreamTokenEvent};
use vivianne_ai::events::Events;
use vivianne_core::{Message, Role};

use crate::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};

const GATEWAY: ProviderSpec = ProviderSpec {
    default_base_url: Some("https://ai-gateway.vercel.sh/v1"),
    api_kind: ApiKind::OpenAiChat,
};

/// One configured provider from the registry (`openai`, `anthropic`, `google`, `groq`...).
#[derive(Debug)]
pub struct Legacy {
    provider: String,
    base_url: Option<String>,
    api_key: Option<String>,
}

impl Legacy {
    /// `base_url` overrides the registry's default (needed for gateways and local servers).
    #[must_use]
    pub fn new(provider: &str, base_url: Option<String>, api_key: Option<String>) -> Self {
        Self {
            provider: provider.to_owned(),
            base_url,
            api_key,
        }
    }
}

fn to_context(request: &ChatRequest) -> Context {
    let messages = request
        .messages
        .iter()
        .map(|m: &Message| AiMessage {
            // Tool results are not wired yet; they travel as user text.
            role: if m.role == Role::Assistant { "assistant" } else { "user" }.to_owned(),
            content_blocks: vec![MessageContent::Text { text: m.text.clone() }],
            text: Some(m.text.clone()),
            ..AiMessage::default()
        })
        .collect();
    Context {
        messages,
        system_prompt: request.system.clone(),
        tools: None,
    }
}

#[async_trait]
impl Provider for Legacy {
    fn id(&self) -> &str {
        &self.provider
    }

    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError> {
        // The Vercel AI Gateway is not in the old registry: it is OpenAI chat-compatible.
        let spec = provider_spec(&self.provider)
            .or_else(|| (self.provider == "vercel").then_some(GATEWAY))
            .ok_or_else(|| ProviderError::Http(format!("unknown provider `{}`", self.provider)))?;
        let base_url = resolve_base_url(&spec, self.base_url.as_deref())
            .ok_or_else(|| ProviderError::Http("this provider needs a base URL".to_owned()))?
            .to_owned();
        let events = Events::new();
        let stream_id = format!("s{}", uuid_like());
        // Subscribe before the stream starts, or the first tokens are missed.
        let rx = events.subscribe();
        let context = to_context(request);
        let (model, provider, key) = (request.model.clone(), self.provider.clone(), self.api_key.clone());
        let id = stream_id.clone();
        tokio::spawn(async move {
            match spec.api_kind {
                ApiKind::AnthropicMessages => {
                    anthropic::stream(events, id, model, provider, Some(base_url), context,
                        anthropic::AnthropicOptions { api_key: key, max_tokens: Some(8192), ..Default::default() }).await;
                }
                ApiKind::OpenAiChat => {
                    openai_completions::stream_chat_completions(events, id, &base_url, &model, &provider, context,
                        openai_completions::OpenAICompletionsOptions { api_key: key, ..Default::default() }).await;
                }
                ApiKind::OpenAiResponses => {
                    openai_responses::stream_responses(events, id, &base_url, &model, &provider, context,
                        openai_responses::OpenAIResponsesOptions { api_key: key, ..Default::default() }).await;
                }
                ApiKind::GoogleGenerative => {
                    google::stream(events, id, model, provider, String::new(), false, Some(base_url), context,
                        google::GoogleOptions { api_key: key, max_tokens: Some(8192), ..Default::default() }).await;
                }
                other => {
                    let _ = events.emit("ai-stream-token", StreamTokenEvent {
                        stream_id: id,
                        delta: format!("{other:?} providers are not wired yet"),
                        done: true,
                        message: None,
                        is_thinking: false,
                    });
                }
            }
        });
        let out = async_stream_lite(rx, stream_id);
        Ok(out)
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{n:x}")
}

/// Turn this stream's tokens into events, ending on the `done` token.
fn async_stream_lite(
    rx: tokio::sync::broadcast::Receiver<vivianne_ai::events::Event>,
    stream_id: String,
) -> EventStream {
    Box::pin(futures::stream::unfold(Some(rx), move |state| {
        let stream_id = stream_id.clone();
        async move {
            let mut rx = state?;
            loop {
                match rx.recv().await {
                    Ok((name, payload)) if name == "ai-stream-token" => {
                        let Ok(ev) = serde_json::from_value::<StreamTokenEvent>(payload) else { continue };
                        if ev.stream_id != stream_id {
                            continue;
                        }
                        if ev.done {
                            let failed = ev.message.as_ref().and_then(|m| {
                                (m.stop_reason == "error").then(|| m.error_message.clone().unwrap_or_default())
                            });
                            let item = match failed {
                                Some(e) => Err(ProviderError::Http(e)),
                                None if !ev.delta.is_empty() => Ok(ChatEvent::Text(ev.delta)),
                                None => Ok(ChatEvent::Done),
                            };
                            // A last delta is followed by Done on the next poll.
                            return Some((item, None));
                        }
                        if !ev.is_thinking && !ev.delta.is_empty() {
                            return Some((Ok(ChatEvent::Text(ev.delta)), Some(rx)));
                        }
                    }
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => return Some((Ok(ChatEvent::Done), None)),
                }
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;

    use super::*;

    fn token(id: &str, delta: &str, done: bool) -> StreamTokenEvent {
        StreamTokenEvent { stream_id: id.into(), delta: delta.into(), done, message: None, is_thinking: false }
    }

    #[tokio::test]
    async fn this_streams_text_is_forwarded_and_others_are_ignored() {
        let events = Events::new();
        let stream = async_stream_lite(events.subscribe(), "a".into());
        events.emit("ai-stream-token", token("b", "other", false)).unwrap();
        events.emit("ai-stream-token", token("a", "Hel", false)).unwrap();
        events.emit("ai-stream-token", token("a", "lo", false)).unwrap();
        events.emit("ai-stream-token", token("a", "", true)).unwrap();
        let got: Vec<_> = stream.map(Result::unwrap).collect().await;
        assert_eq!(got, vec![ChatEvent::Text("Hel".into()), ChatEvent::Text("lo".into()), ChatEvent::Done]);
    }
}
