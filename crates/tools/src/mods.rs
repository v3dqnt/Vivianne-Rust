#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `mods` module, which the language-server code asks
//! one question: has the user trusted this project folder?
//!
//! Nothing is trusted until the new app has a trust store, so language servers
//! a project defines for itself do not run; the built-in ones do.

pub fn project_trusted(_project: &str) -> bool {
    false
}
