//! Where the provider layer keeps its files: under `~/.viv`, where the old app
//! kept them, so existing data is read in place. This never renames or moves
//! anything (ADR 0014).

use std::path::{Path, PathBuf};

use serde::Serialize;

/// `~/.viv`.
pub fn viv_home() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "no home folder".to_owned())?;
    Ok(home.join(".viv"))
}

/// A path under `~/.viv`, e.g. `viv_home_path(&["chatgpt"])`.
#[must_use]
pub fn viv_home_path(parts: &[&str]) -> PathBuf {
    let mut path = viv_home().unwrap_or_else(|_| std::env::temp_dir().join(".viv"));
    for part in parts {
        path.push(part);
    }
    path
}

/// Write `value` as JSON through a temp file and a rename, so a crash never
/// leaves half a file.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_write_replaces_the_file_and_leaves_no_temp_behind() {
        let dir = std::env::temp_dir().join(format!("viv-ai-paths-{}", std::process::id()));
        let file = dir.join("a.json");
        write_json_atomic(&file, &serde_json::json!({"n": 1})).unwrap();
        write_json_atomic(&file, &serde_json::json!({"n": 2})).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains('2'));
        assert!(!file.with_extension("tmp").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_paths_live_under_dot_viv() {
        assert!(viv_home_path(&["chatgpt"]).ends_with(Path::new(".viv").join("chatgpt")));
    }
}
