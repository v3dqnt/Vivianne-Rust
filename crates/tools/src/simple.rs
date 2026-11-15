#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! A small tool shape for the stand-in tools (shell, write, edit), adapted to
//! the original `Tool` trait. Goes away once those tools are copied too.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::builtin::{Tool, ToolResult};

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("bad arguments: {0}")]
    BadArgs(String),
    #[error("{0}")]
    Failed(String),
}

pub struct SimpleOutput {
    pub text: String,
    pub details: Option<Value>,
}

impl SimpleOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            details: None,
        }
    }
}

#[async_trait]
pub trait SimpleTool: Send + Sync {
    fn name(&self) -> &str;
    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError>;
}

#[async_trait]
impl<T: SimpleTool> Tool for T {
    fn name(&self) -> &str {
        SimpleTool::name(self)
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        match self.call(args).await {
            Ok(out) => ToolResult {
                content: out.text,
                is_error: false,
                details: out.details,
            },
            Err(e) => ToolResult {
                content: e.to_string(),
                is_error: true,
                details: None,
            },
        }
    }
}

pub(crate) fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    args[key]
        .as_str()
        .ok_or_else(|| ToolError::BadArgs(format!("`{key}` must be a string")))
}

/// Definitions for the stand-in tools.
pub fn definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "shell",
            "description": if cfg!(windows) {
                "Run a PowerShell command in the project folder. Returns exit code and output."
            } else {
                "Run a sh command in the project folder. Returns exit code and output."
            },
            "parameters": {
                "type": "object",
                "properties": {
                    "command": {"type": "string"},
                    "timeout_secs": {"type": "integer", "description": "default 120, max 600"}
                },
                "required": ["command"]
            }
        }),
        json!({
            "name": "write",
            "description": "Write content to a file, creating it if it doesn't exist. This REPLACES the file's entire content: use it to create new files or rewrite one wholesale. To change part of an existing file, use edit instead.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path of the file"},
                    "content": {"type": "string", "description": "Content to write (always the complete file, UTF-8)"}
                },
                "required": ["path", "content"]
            }
        }),
        json!({
            "name": "edit",
            "description": "Replace a specific substring in a file with new text. old_string must match the file's content EXACTLY, including indentation, and must appear exactly once: include enough surrounding lines to make it unique.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Relative path of the file"},
                    "old_string": {"type": "string", "description": "Exact existing text to replace (must be unique in the file)"},
                    "new_string": {"type": "string", "description": "New content to insert"}
                },
                "required": ["path", "old_string", "new_string"]
            }
        }),
    ]
}
