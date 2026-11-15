//! File tools: read, write (atomic) and edit (exact single match).

use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{Tool, ToolError, Workspace, string_arg};

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
pub struct ReadFile(Workspace);

impl ReadFile {
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        Self(ws)
    }
}

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read"
    }
    fn description(&self) -> &'static str {
        "Read a text file from the project."
    }
    fn parameters(&self) -> Value {
        json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]})
    }
    async fn call(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.0.resolve(string_arg(args, "path")?)?;
        tokio::fs::read_to_string(path).await.map_err(failed)
    }
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
impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write"
    }
    fn description(&self) -> &'static str {
        "Create or replace a file with the given content."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["path", "content"]
        })
    }
    async fn call(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.0.resolve(string_arg(args, "path")?)?;
        let content = string_arg(args, "content")?;
        write_atomic(&path, content).await?;
        Ok(format!("wrote {} bytes", content.len()))
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
impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit"
    }
    fn description(&self) -> &'static str {
        "Replace `old` with `new` in a file. `old` must match exactly once."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old": {"type": "string"},
                "new": {"type": "string"}
            },
            "required": ["path", "old", "new"]
        })
    }
    async fn call(&self, args: &Value) -> Result<String, ToolError> {
        let path = self.0.resolve(string_arg(args, "path")?)?;
        let (old, new) = (string_arg(args, "old")?, string_arg(args, "new")?);
        let text = tokio::fs::read_to_string(&path).await.map_err(failed)?;
        match text.matches(old).count() {
            1 => {
                write_atomic(&path, &text.replacen(old, new, 1)).await?;
                Ok("edited".to_owned())
            }
            0 => Err(ToolError::Failed("`old` not found".to_owned())),
            n => Err(ToolError::Failed(format!(
                "`old` matches {n} times; add context"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Workspace {
        let dir = std::env::temp_dir().join(format!("viv-files-{name}-{}", std::process::id()));
        Workspace::new(&dir)
    }

    #[tokio::test]
    async fn write_then_read_then_edit() {
        let ws = scratch("flow");
        WriteFile(ws.clone())
            .call(&json!({"path": "a/b.txt", "content": "one two"}))
            .await
            .unwrap();
        let read = ReadFile(ws.clone())
            .call(&json!({"path": "a/b.txt"}))
            .await
            .unwrap();
        assert_eq!(read, "one two");
        EditFile(ws.clone())
            .call(&json!({"path": "a/b.txt", "old": "two", "new": "2"}))
            .await
            .unwrap();
        assert_eq!(
            ReadFile(ws.clone())
                .call(&json!({"path": "a/b.txt"}))
                .await
                .unwrap(),
            "one 2"
        );
        let _ = tokio::fs::remove_dir_all(ws.root()).await;
    }

    #[tokio::test]
    async fn edit_refuses_ambiguous_and_missing_matches() {
        let ws = scratch("edit");
        WriteFile(ws.clone())
            .call(&json!({"path": "f", "content": "aa"}))
            .await
            .unwrap();
        let edit = EditFile(ws.clone());
        assert!(
            edit.call(&json!({"path": "f", "old": "a", "new": "b"}))
                .await
                .is_err()
        );
        assert!(
            edit.call(&json!({"path": "f", "old": "z", "new": "b"}))
                .await
                .is_err()
        );
        let _ = tokio::fs::remove_dir_all(ws.root()).await;
    }

    #[tokio::test]
    async fn reading_outside_the_workspace_is_refused() {
        let ws = scratch("fence");
        assert!(
            ReadFile(ws)
                .call(&json!({"path": "../../etc/passwd"}))
                .await
                .is_err()
        );
    }
}
