//! Request context: things added to a conversation, beside the person's words, that the model should see. Workspace
//! instructions, the clock, the terminal's location, other sessions the person mentions, and `@file` paths.
//!
//! Each piece is a durable user-role message (`vivianne_core::context::ContextSource` says whose), so it is saved, replayed
//! and condensed like any other history. Everything here is pure or goes through a small trait for the disk, a process or the
//! saved threads, so its rules are tested with made-up inputs. The agent loop (`vivianne-runtime`) calls it each step.

// Ports of byte-budget and clock arithmetic: the casts are bounded by the budgets and the maps are private caches.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::implicit_hasher,
    clippy::manual_midpoint,
    clippy::too_many_lines
)]

pub mod dev;
pub mod file_ref;
pub mod instructions;
pub mod session_ref;
pub mod time;
pub mod tmux;
