//! What the agent loop sees of a model provider: one small trait, [`Provider`].
//!
//! Every real provider is Vivianne's old provider layer (`vivianne-ai`), reached through
//! [`ai_provider::AiProvider`]. [`env`] picks one from environment variables.

pub mod acp;
pub mod ai_provider;
pub mod env;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use vivianne_core::{Message, Thinking, ToolCall};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// The provider could not be reached or was set up wrongly (unknown provider, no base URL).
    #[error("{0}")]
    Unavailable(String),
    /// The provider answered with a failure.
    #[error("{0}")]
    Failed(String),
}

/// A tool the model may call, described for the provider.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments.
    pub parameters: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub thinking: Thinking,
}

/// What a provider streams back. Tool calls arrive complete, never as fragments.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    Text(String),
    ToolCall(ToolCall),
    Done,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<ChatEvent, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError>;
}
