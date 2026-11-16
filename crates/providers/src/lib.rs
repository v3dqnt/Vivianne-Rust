//! Model providers behind one small trait (ADR 0007).
//!
//! Two adapters cover the field: [`openai::OpenAiCompatible`] (OpenAI, Google's
//! compatible endpoint, local servers like Ollama or llama.cpp) and
//! [`anthropic::Anthropic`].

pub mod anthropic;
pub mod env;
pub mod legacy;
pub mod openai;
mod sse;

use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use vivianne_core::{Message, ToolCall};

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Http(String),
    #[error("provider returned {status}: {body}")]
    Api { status: u16, body: String },
    #[error("could not read provider stream: {0}")]
    Parse(String),
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
