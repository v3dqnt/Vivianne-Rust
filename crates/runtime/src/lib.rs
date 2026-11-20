//! The single agent loop, used by chats and (later) 24/7 agents (ADR 0006).
//!
//! One turn: stream the model, run the tools it asks for, feed results back,
//! repeat until it answers without tools. No UI types here: callers watch
//! [`AgentEvent`]s on a channel. Retry, compaction and repetition guards from
//! the old loop are ported on top of this: retrying, backing off and falling back to
//! other models live in [`resilience`].

mod agent;
mod attempts;
mod compact;
pub mod context;
mod event;
mod history;
mod prompt;
pub mod resilience;

pub use agent::{Agent, AgentError, Target};
pub use event::AgentEvent;
pub use prompt::coding_system_prompt;
