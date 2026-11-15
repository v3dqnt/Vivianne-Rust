#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in `write` and `edit` (atomic write, exact single-match edit), confined
//! to the project folder. Replaced by the original tools later.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::simple::{SimpleOutput, SimpleTool, ToolError, string_arg};
use crate::workspace::Workspace;

fn failed(e: impl std::fmt::Display) -> ToolError {
    ToolError::Failed(e.to_string())
}

/// Write via a temp file and rename so a crash never leaves half a file.
async fn write_atomic(path: &Path, content: &str) -> Result<(), ToolError> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await.map_err(failed)?;
    }
    let tmp = path.with_extension("viv-tmp");
    tokio::fs::write(&tmp, content).await.map_err(failed)?;
    tokio::fs::rename(&tmp, path).await.map_err(failed)
}

#[derive(Debug)]
pub struct WriteFile(Workspace);

impl WriteFile {
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        Self(ws)
    }
}

#[async_trait]
impl SimpleTool for WriteFile {
    fn name(&self) -> &str {
        "write"
    }

    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError> {
        let path = self.0.resolve(string_arg(args, "path")?)?;
        let content = string_arg(args, "content")?;
        write_atomic(&path, content).await?;
        Ok(SimpleOutput {
            text: format!("wrote {} bytes", content.len()),
            details: Some(json!({"path": self.0.display(&path), "bytes": content.len()})),
        })
    }
}

#[derive(Debug)]
pub struct EditFile(Workspace);

impl EditFile {
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        Self(ws)
    }
}

#[async_trait]
impl SimpleTool for EditFile {
    fn name(&self) -> &str {
        "edit"
    }

    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError> {
        let path = self.0.resolve(string_arg(args, "path")?)?;
        let (old, new) = (
            string_arg(args, "old_string")?,
            string_arg(args, "new_string")?,
        );
        let text = tokio::fs::read_to_string(&path).await.map_err(failed)?;
        match text.matches(old).count() {
            1 => {
                write_atomic(&path, &text.replacen(old, new, 1)).await?;
                Ok(SimpleOutput {
                    text: "edited".to_owned(),
                    details: Some(json!({"path": self.0.display(&path), "old": old, "new": new})),
                })
            }
            0 => Err(ToolError::Failed("`old_string` not found".to_owned())),
            n => Err(ToolError::Failed(format!(
                "`old_string` matches {n} times; add context"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::Tool;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("viv-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Workspace::new(&dir)
    }

    #[tokio::test]
    async fn write_then_edit_changes_the_file() {
        let ws = scratch("flow");
        let out = WriteFile(ws.clone())
            .execute(&json!({"path": "a/b.txt", "content": "one two"}), "")
            .await;
        assert!(!out.is_error, "{}", out.content);
        let edit = json!({"path": "a/b.txt", "old_string": "two", "new_string": "2"});
        assert!(!EditFile(ws.clone()).execute(&edit, "").await.is_error);
        let text = std::fs::read_to_string(ws.root().join("a").join("b.txt")).unwrap();
        assert_eq!(text, "one 2");
        let _ = std::fs::remove_dir_all(ws.root());
    }

    #[tokio::test]
    async fn edit_refuses_ambiguous_and_missing_matches() {
        let ws = scratch("edit");
        WriteFile(ws.clone())
            .execute(&json!({"path": "f", "content": "aa"}), "")
            .await;
        let edit = EditFile(ws.clone());
        let ambiguous = json!({"path": "f", "old_string": "a", "new_string": "b"});
        let missing = json!({"path": "f", "old_string": "z", "new_string": "b"});
        assert!(edit.execute(&ambiguous, "").await.is_error);
        assert!(edit.execute(&missing, "").await.is_error);
        let _ = std::fs::remove_dir_all(ws.root());
    }

    #[tokio::test]
    async fn writing_outside_the_workspace_is_refused() {
        let ws = scratch("fence");
        let out = WriteFile(ws)
            .execute(&json!({"path": "../../x", "content": "no"}), "")
            .await;
        assert!(out.is_error);
    }
}
