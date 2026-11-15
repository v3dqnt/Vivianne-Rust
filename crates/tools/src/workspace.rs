//! The folder an agent may touch. Paths outside it are refused.
//! Asking the user instead of refusing arrives with the consent UI.

use std::path::{Component, Path, PathBuf};

use crate::ToolError;

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

    /// Resolve `path` (relative to the root, or absolute) and check it stays inside.
    // ponytail: lexical check only, a symlink inside the root can still point out of it.
    pub fn resolve(&self, path: &str) -> Result<PathBuf, ToolError> {
        let mut resolved = PathBuf::new();
        for part in self.root.join(path).components() {
            match part {
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::CurDir => {}
                other => resolved.push(other),
            }
        }
        if resolved.starts_with(&self.root) {
            Ok(resolved)
        } else {
            Err(ToolError::Failed(format!(
                "`{path}` is outside the workspace"
            )))
        }
    }
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
}
