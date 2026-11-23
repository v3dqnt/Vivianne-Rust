#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the one function the copied code uses from the original
//! `agent::tools::exec`: the user's settings file, read as it is on disk.

pub async fn read_settings_json() -> serde_json::Value {
    let path = crate::agent::system::paths::viv_home_path(&["agent", "settings.json"]);
    match tokio::fs::read_to_string(path).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or(serde_json::Value::Null),
        Err(_) => serde_json::Value::Null,
    }
}

/// Longest text a tool hands back when it has no tighter limit of its own.
pub const MAX_UNBOUNDED_TOOL_CHARS: usize = 64 * 1024;

/// `text` cut to at most `max` characters, with a note saying so.
pub fn clip_tool_text(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max).collect();
    format!(
        "{kept}
… (clipped at {max} characters)"
    )
}
