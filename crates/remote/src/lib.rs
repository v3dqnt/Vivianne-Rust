//! Several machines, one window: a port of herdr's multi-machine system. Each machine runs a server that owns its
//! threads and agent runs; this app is a client that reaches it over the system `ssh` through a small bridge,
//! remembers the machines in a catalog, reconnects by itself, and shows what each is doing.
//!
//! Everything about a connection that can be a plain function is one, so its rules are tested with made-up inputs:
//! the catalog, the `ssh` options, the frames, the retry and health clocks, the install choices, and the screen
//! rules for terminal agents. See the README.

pub mod acp;
pub mod bridge;
pub mod catalog;
pub mod client;
pub mod detect;
pub mod health;
pub mod install;
pub mod meta;
pub mod pane;
pub mod panes;
pub mod persist;
pub mod protocol;
pub mod server;
pub mod socket;
pub mod ssh;
pub mod supervisor;

/// What the client asks of a server in its hello.
pub const CAP_WANTED: &[&str] = &[
    protocol::CAP_SURFACE_INTEREST,
    protocol::CAP_HEALTH_CHECK,
    protocol::CAP_SESSION_CREDENTIAL,
    protocol::CAP_AGENT_RUNS,
    protocol::CAP_TERMINAL_PANES,
];
