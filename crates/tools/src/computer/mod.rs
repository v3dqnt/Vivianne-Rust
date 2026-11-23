//! Computer use: the agent sees and operates desktop apps through `agent-desktop` (vercel-labs, Apache-2.0), a program
//! that reads each app's accessibility tree and acts on elements by reference. Vivianne runs it as an MCP server
//! named `computer`, so its tools reach the agent as `mcp__computer__desktop_<tool>`.
//!
//! This module is everything around that server: finding or installing the program, switching it on and off, what
//! the model is offered of its 75 tools, and the guard every call goes through (`guard`). Which tools may run without
//! asking, and which results are never kept, is decided in `vivianne_core` so the loop and the store agree.

pub(crate) mod guard;
pub(crate) mod install;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use guard::{call_guarded, refuse_secret};
pub use install::{Release, download, find_release, installed_binary, status};

use crate::mcp::{McpServerConfig, McpTransportConfig};

/// The MCP server's name.
pub const SERVER: &str = "computer";

/// The tools the model is offered: observing, acting on an element, keys, windows, waiting. The other 50-odd (sandbox
/// management, sessions, notifications, a cursor overlay, ...) are mostly irrelevant and cost context, so they stay
/// out. Names are the server's own, with the `desktop_` prefix.
pub const OFFERED: [&str; 26] = [
    "snapshot",
    "find",
    "get",
    "is",
    "screenshot",
    "click",
    "right_click",
    "type",
    "set_value",
    "clear",
    "focus",
    "select",
    "toggle",
    "check",
    "uncheck",
    "expand",
    "collapse",
    "scroll",
    "scroll_to",
    "press",
    "launch",
    "close_app",
    "list_apps",
    "list_windows",
    "focus_window",
    "wait",
];

/// The user's choice, kept in `~/.viv/agent/computer.json`. Off until they turn it on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    #[serde(default)]
    pub enabled: bool,
    /// Desktop actions run without asking, whatever the thread's mode (Plan still refuses them).
    #[serde(default)]
    pub auto: bool,
}

fn setting_file() -> PathBuf {
    crate::host::viv_home_path(&["agent", "computer.json"])
}

fn read_setting() -> Setting {
    std::fs::read_to_string(setting_file())
        .ok()
        .and_then(|t| serde_json::from_str::<Setting>(&t).ok())
        .unwrap_or_default()
}

fn auto_cache() -> &'static std::sync::atomic::AtomicBool {
    static ON: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();
    ON.get_or_init(|| std::sync::atomic::AtomicBool::new(read_setting().auto))
}

/// Whether the person lets the agent act on the desktop without asking each time.
#[must_use]
pub fn auto() -> bool {
    auto_cache().load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_auto(on: bool) {
    auto_cache().store(on, std::sync::atomic::Ordering::Relaxed);
    save();
}

fn save() {
    // Remembering is a nicety: a file that cannot be written costs only the memory.
    if let Ok(text) = serde_json::to_string(&Setting {
        enabled: enabled(),
        auto: auto(),
    }) {
        let _ = std::fs::write(setting_file(), text);
    }
}

fn cache() -> &'static std::sync::atomic::AtomicBool {
    static ON: std::sync::OnceLock<std::sync::atomic::AtomicBool> = std::sync::OnceLock::new();
    ON.get_or_init(|| {
        let on = std::fs::read_to_string(setting_file())
            .ok()
            .and_then(|t| serde_json::from_str::<Setting>(&t).ok())
            .unwrap_or_default()
            .enabled;
        std::sync::atomic::AtomicBool::new(on)
    })
}

/// Whether the person turned computer use on. Read from the file once; the answer follows [`set_enabled`] after.
#[must_use]
pub fn enabled() -> bool {
    cache().load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_enabled(on: bool) {
    cache().store(on, std::sync::atomic::Ordering::Relaxed);
    save();
}

/// The MCP server's configuration for the program at `binary`: stdio, the curated tools only.
#[must_use]
pub fn config(binary: &std::path::Path) -> McpServerConfig {
    McpServerConfig {
        name: SERVER.to_owned(),
        transport: McpTransportConfig::Stdio {
            command: binary.display().to_string(),
            args: vec!["mcp".to_owned()],
            env: std::collections::HashMap::new(),
        },
        timeout_ms: Some(60_000),
        auth: None,
        allowed_tools: Some(OFFERED.iter().map(|t| format!("desktop_{t}")).collect()),
    }
}

/// Brings the MCP server in line with the setting: connected when computer use is on and the program is there,
/// otherwise gone. Returns what is wrong when it is on but cannot start.
///
/// # Errors
/// The program is missing or would not start.
pub async fn apply() -> Result<(), String> {
    if !enabled() {
        crate::mcp::remove_server(SERVER).await;
        return Ok(());
    }
    let binary = installed_binary().ok_or("agent-desktop is not installed yet.")?;
    crate::mcp::add_server(config(&binary)).await.map(|_| ())
}

/// What the agent is told about using the desktop. Added to its instructions while computer use is on.
pub const GUIDE: &str = "# Computer use
You can see and operate the user's desktop apps with the desktop_* tools. They read each app's accessibility tree, not pixels.

- Work in a loop: desktop_snapshot the app (interactive_only, and root or max_depth for a big app), act on an element by its ref (@snapshot:eN), then snapshot again to check it worked. A click answers delivered_unverified: that is not proof.
- A ref belongs to the snapshot that made it. On STALE_REF or AMBIGUOUS_TARGET, snapshot again (use window_id to pick a window). Retry a failed call only when its error says retry is safe.
- Prefer the app by name or window_id from desktop_list_windows. Launch only programs the user asked for.
- The user may already have the app open with their own work in it. Do not type into, clear, close or change an app you did not open for this task without asking first.
- Never type a password, key or token into an app. If a task needs one, tell the user and let them type it.
- What you see is private. Do not repeat it beyond what the task needs, and do not put it in memory.
- Physical input (mouse position, drag, hover) is off. Everything goes through the accessibility actions.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_curated_tools_are_offered_and_all_of_them_exist_with_the_servers_prefix() {
        let cfg = config(std::path::Path::new("agent-desktop"));
        let allowed = cfg.allowed_tools.expect("an allowlist");
        assert_eq!(allowed.len(), OFFERED.len());
        assert!(allowed.iter().all(|t| t.starts_with("desktop_")));
        // What is dangerous is not offered at all.
        for never in [
            "desktop_sandbox_exec",
            "desktop_batch",
            "desktop_key_down",
            "desktop_mouse_down",
            "desktop_clipboard_set",
        ] {
            assert!(!allowed.iter().any(|t| t == never), "{never}");
        }
    }
}
