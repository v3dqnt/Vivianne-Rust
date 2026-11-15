#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in for the one function the copied code uses from the original
//! `agent::runtime::commands`.

use std::path::Path;

/// Write `content` to `path` through a temp file and a rename.
pub fn write_session_file_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}
