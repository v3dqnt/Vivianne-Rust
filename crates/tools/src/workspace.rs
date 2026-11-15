//! The folder the stand-in write/edit/shell tools may touch. Paths outside are
//! refused; asking the user instead comes with the consent UI.

use std::path::{Component, Path, PathBuf};

use crate::simple::ToolError;

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve `path` for writing: it must stay inside the workspace.
    // ponytail: lexical check only, a symlink inside the root can still point out of it.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let resolved = normalize(&self.root.join(path));
        if resolved.starts_with(&self.root) {
            Ok(resolved)
        } else {
            Err(ToolError::Failed(format!(
                "`{path}` is outside the workspace"
            )))
        }
    }

    /// `path` as shown to the model and the UI: relative to the workspace when it is inside.
    #[must_use]
    pub fn display(&self, path: &Path) -> String {
        let shown = path.strip_prefix(&self.root).unwrap_or(path);
        let text = shown.to_string_lossy().replace('\\', "/");
        if text.is_empty() {
            ".".to_owned()
        } else {
            text
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> Workspace {
        Workspace::new(&std::env::temp_dir().join("viv-ws"))
    }

    #[test]
    fn relative_paths_stay_inside() {
        assert!(ws().resolve("src/a.rs").unwrap().ends_with("src/a.rs"));
        assert!(ws().resolve("./x/../y").unwrap().ends_with("y"));
    }

    #[test]
    fn parent_escapes_are_refused() {
        assert!(ws().resolve("../secret").is_err());
        assert!(ws().resolve("a/../../secret").is_err());
    }

    #[test]
    fn absolute_paths_outside_are_refused() {
        let outside = std::env::temp_dir().join("elsewhere");
        assert!(ws().resolve(outside.to_str().unwrap()).is_err());
    }

    #[test]
    fn display_is_relative_with_forward_slashes() {
        let w = ws();
        assert_eq!(w.display(&w.root().join("src").join("a.rs")), "src/a.rs");
        assert_eq!(w.display(w.root()), ".");
    }
}
