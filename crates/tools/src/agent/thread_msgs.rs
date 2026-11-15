#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `agent::thread_msgs`.

use super::orchestration::AppHandle;

pub async fn deliver_if_running(
    _app: &AppHandle,
    _thread: &str,
    _note: &str,
) -> Result<(), String> {
    Err("no running thread".to_owned())
}
