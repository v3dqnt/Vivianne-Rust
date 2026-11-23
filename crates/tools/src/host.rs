//! What the copied tools need from the old app, in one place: where files live under `~/.viv`, the settings
//! file, an atomic write, and the two seams the new runtime has not filled yet (the running thread, and
//! whether a project folder is trusted). Each seam answers "none"/`false` today; the copied code already
//! handles that.

use std::path::{Path, PathBuf};

/// `~/.viv`.
pub fn viv_home() -> Result<PathBuf, String> {
    // `VIV_HOME` replaces `~/.viv`. It must be an absolute folder that is not a root or the home folder itself, so
    // a typo cannot make the app write into the wrong place.
    if let Some(custom) = std::env::var_os("VIV_HOME").map(PathBuf::from)
        && custom.is_absolute()
        && custom.parent().is_some()
        && !is_home_folder(&custom)
    {
        return Ok(custom);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "no home folder".to_owned())?;
    Ok(home.join(".viv"))
}

fn is_home_folder(path: &Path) -> bool {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|h| Path::new(&h) == path)
}

/// A path under `~/.viv`, e.g. `viv_home_path(&["repo-cache"])`.
pub fn viv_home_path(parts: &[&str]) -> PathBuf {
    let mut path = viv_home().unwrap_or_else(|_| std::env::temp_dir().join(".viv"));
    for part in parts {
        path.push(part);
    }
    path
}

/// The user's settings file, read as it is on disk.
pub async fn read_settings_json() -> serde_json::Value {
    let path = viv_home_path(&["agent", "settings.json"]);
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

/// Write `content` to `path` through a temp file and a rename.
pub fn write_session_file_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Has the user trusted this project folder? Nothing is until the app has a trust store, so language servers
/// and debug adapters a project defines for itself do not run; the built-in ones do.
pub fn project_trusted(_project: &str) -> bool {
    false
}

/// Which thread the current turn belongs to. None until the runtime tells the tools.
pub fn turn_thread() -> Option<String> {
    None
}
