//! What is learned about each machine's operating system, kept in `~/.viv/client/machine-metadata.json`. It picks the
//! remote command that starts the bridge (a POSIX shell command, or PowerShell for a Windows host). The cache is
//! optional: a missing or unreadable file only means the machine is treated as a POSIX host until it is detected.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Meta {
    pub os: String,
    pub arch: String,
}

fn path() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["client", "machine-metadata.json"])
}

fn load_from(path: &std::path::Path) -> HashMap<String, Meta> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// What is known of a machine, by its id.
#[must_use]
pub fn get(id: &str) -> Option<Meta> {
    load_from(&path()).remove(id)
}

/// Remembers a machine's platform. Failing to write it costs only a detection next time.
pub fn put(id: &str, meta: Meta) {
    let mut all = load_from(&path());
    all.insert(id.to_owned(), meta);
    let _ = vivianne_ai::paths::write_json_atomic(&path(), &all);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_broken_cache_is_just_empty() {
        let dir = std::env::temp_dir().join(format!("viv-meta-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("creates");
        assert!(load_from(&dir.join("none.json")).is_empty());
        std::fs::write(dir.join("bad.json"), "{ nope").expect("writes");
        assert!(load_from(&dir.join("bad.json")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
