#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `agent::orchestration`: which thread the current turn
//! belongs to and the app handle for delivering notes into it. The new runtime has
//! neither yet, so both are absent; the copied code already handles that.

pub struct AppHandle;

pub fn turn_thread() -> Option<String> {
    None
}

pub fn app() -> Option<AppHandle> {
    None
}
