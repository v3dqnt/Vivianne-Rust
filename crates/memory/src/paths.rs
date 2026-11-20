//! Where a project keeps its own files: `<project>/.viv/...`.

use std::path::{Path, PathBuf};

/// `<project>/.viv/<rel...>`, moving an older `<project>/.vivianne/<rel...>` here the first time, so a project keeps
/// one app folder, not two.
pub fn project_viv_path(root: &Path, rel: &[&str]) -> PathBuf {
    let new = rel.iter().fold(root.join(".viv"), |p, part| p.join(part));
    let old = rel.iter().fold(root.join(".vivianne"), |p, part| p.join(part));
    if !new.exists() && old.exists() {
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::rename(&old, &new);
    }
    new
}
