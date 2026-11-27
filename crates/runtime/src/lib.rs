//! The single agent loop, used by chats and (later) 24/7 agents (ADR 0006).
//!
//! One turn: stream the model, run the tools it asks for, feed results back,
//! repeat until it answers without tools. No UI types here: callers watch
//! [`AgentEvent`]s on a channel. Retry, compaction and repetition guards from
//! the old loop are ported on top of this: retrying, backing off and falling back to
//! other models live in [`resilience`].

mod agent;
pub mod attachments;
mod attempts;
pub mod automation;
pub mod changes;
pub mod checkpoints;
mod compact;
pub mod config;
pub mod context;
pub mod deliverables;
mod event;
pub mod archive;
pub mod export;
pub mod export_bundle;
pub mod formats;
pub mod history;
pub mod lease;
pub mod log;
pub mod ledger;
pub mod orchestration;
pub mod profiles;
pub mod projects;
mod prompt;
pub mod resilience;
pub mod sections;
pub mod session;
pub mod projection;
pub mod query;
pub mod quotes;
pub mod session_stats;
pub mod store;
pub mod thread_tools;
pub mod thread_refs;
pub mod title;
pub mod workspace_context;

pub use agent::{Agent, AgentError, Checkpoint, ContextSource, ResultFilter, Target};
pub use event::AgentEvent;
pub use prompt::coding_system_prompt;
