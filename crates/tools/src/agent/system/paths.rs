#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the original `agent::system::paths`: where Vivianne keeps its files.
//! The original also renames old folders on first use. This one never does: the new
//! app reads existing data in place (ADR 0014) and leaves moving it to a migration.

use std::path::PathBuf;

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

fn is_home_folder(path: &std::path::Path) -> bool {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|h| std::path::Path::new(&h) == path)
}

/// A path under `~/.viv`, e.g. `viv_home_path(&["repo-cache"])`.
pub fn viv_home_path(parts: &[&str]) -> PathBuf {
    let mut path = viv_home().unwrap_or_else(|_| std::env::temp_dir().join(".viv"));
    for part in parts {
        path.push(part);
    }
    path
}
