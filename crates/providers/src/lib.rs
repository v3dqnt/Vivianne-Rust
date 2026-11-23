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
    /// Images to send with the user messages named by `ImagePart::message`: made at request time from stored
    /// attachments, never kept in the conversation.
    pub images: Vec<ImagePart>,
}

/// One picture for one message of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePart {
    /// Index into `ChatRequest::messages`.
    pub message: usize,
    pub media_type: String,
    pub base64: String,
    /// What the model is told beside the picture: its stable id and its real size.
    pub label: String,
}

/// What one model call used: tokens, and what they cost when that is known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spent {
    pub provider: String,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// `None` when the model has no known price: unknown, never zero.
    pub cost: Option<f64>,
}

impl Spent {
    /// Adds `other`'s tokens and cost to this. A cost stays known only while both are.
    pub fn add(&mut self, other: &Spent) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.cost = self.cost.zip(other.cost).map(|(a, b)| a + b);
    }
}

/// What a provider streams back. Tool calls arrive complete, never as fragments.
#[derive(Debug, Clone, PartialEq)]
pub enum ChatEvent {
    Text(String),
    ToolCall(ToolCall),
    /// What the call used, sent once as it ends. The provider only reports it; whoever knows which
    /// thread it was for records it.
    Usage(Spent),
    Done,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<ChatEvent, ProviderError>> + Send>>;

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &str;
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, ProviderError>;
}
