//! The single agent loop, used by chats and (later) 24/7 agents (ADR 0006).
//!
//! One turn: stream the model, run the tools it asks for, feed results back,
//! repeat until it answers without tools. No UI types here: callers watch
//! [`AgentEvent`]s on a channel. Retry, compaction and repetition guards from
//! the old loop are ported on top of this in later steps.

mod agent;
mod event;

pub use agent::{Agent, AgentError};
pub use event::AgentEvent;

/// Default instructions for the coding agent.
pub const CODING_SYSTEM_PROMPT: &str =
    "You are Vivianne, a coding agent. Use the tools to inspect and change the project. Be brief.";
