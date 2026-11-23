//! Multiplayer: two people working on one project from two machines, each with their own agent.
//!
//! The two machines connect directly (an encrypted peer-to-peer connection through `iroh`); there is no server.
//! A project is paired with one partner's code. Each side's agent is told, briefly, what the other did; the code is
//! kept in step through a shared git remote or, without one, over the same connection; and the two can talk.
//! The protocol is the old app's, so a pairing between this app and the old one works.
//!
//! - `identity`: this machine's key, whose public half is the code to share.
//! - `protocol`: the frames on the wire.
//! - `collab`: activity summaries, the note the agent reads, sanitising, and git sync. No network here.
//! - `state`: the connection, the chat, and the hooks around an agent run.

pub mod collab;
pub mod identity;
pub mod protocol;
mod state;

pub use state::{ChatEntry, Event, Multiplayer, Paired, Projects, RunGuard, Status, global};
