#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `agent::system::paths`: where Vivianne keeps its files.
//! The original also renames old folders on first use. This one never does: the new
//! app reads existing data in place (ADR 0014) and leaves moving it to a migration.

use std::path::PathBuf;

/// `~/.viv`.
pub fn viv_home() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .ok_or_else(|| "no home folder".to_owned())?;
    Ok(home.join(".viv"))
}

/// A path under `~/.viv`, e.g. `viv_home_path(&["repo-cache"])`.
pub fn viv_home_path(parts: &[&str]) -> PathBuf {
    let mut path = viv_home().unwrap_or_else(|_| std::env::temp_dir().join(".viv"));
    for part in parts {
        path.push(part);
    }
    path
}
