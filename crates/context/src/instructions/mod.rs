//! Workspace instructions: `AGENTS.md`-style files from the user's home and from the project, entering a conversation as
//! durable user-role messages. See the crate README for the behaviour.

pub mod config;
pub mod engine;
pub mod files;
pub mod render;
pub mod state;

pub use config::{Config, Resolved};
pub use engine::Engine;
