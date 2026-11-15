#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
//! Stand-in `shell`: run a command in the project folder with a timeout.
//! Replaced by the original `bash` and `powershell` tools later.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::simple::{SimpleOutput, SimpleTool, ToolError, string_arg};
use crate::workspace::Workspace;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct Shell {
    workspace: Workspace,
}

impl Shell {
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace }
    }
}

#[async_trait]
impl SimpleTool for Shell {
    fn name(&self) -> &str {
        "shell"
    }

    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError> {
        let command = string_arg(args, "command")?;
        let secs = args["timeout_secs"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(5, MAX_TIMEOUT_SECS);
        let mut cmd = shell_command(command);
        cmd.current_dir(self.workspace.root())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        let run = tokio::time::timeout(Duration::from_secs(secs), cmd.output());
        let output = run
            .await
            .map_err(|_| ToolError::Failed(format!("timed out after {secs}s")))?
            .map_err(|e| ToolError::Failed(e.to_string()))?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        let exit_code = output.status.code();
        Ok(SimpleOutput {
            text: format!("exit {}\n{}", exit_code.unwrap_or(-1), truncate(text)),
            details: Some(json!({"command": command, "exit_code": exit_code})),
        })
    }
}

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// Keep the tail: errors and results are at the end.
fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_BYTES {
        let mut cut = text.len() - MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(cut) {
            cut += 1;
        }
        text = format!("[...earlier output dropped]\n{}", &text[cut..]);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::Tool;

    fn shell() -> Shell {
        Shell::new(Workspace::new(&std::env::temp_dir()))
    }

    #[tokio::test]
    async fn runs_a_command_and_reports_exit_code() {
        let out = shell().execute(&json!({"command": "echo hello"}), "").await;
        assert!(!out.is_error);
        assert!(out.content.starts_with("exit 0"), "{}", out.content);
        assert!(out.content.contains("hello"), "{}", out.content);
        assert_eq!(out.details.unwrap()["exit_code"], 0);
    }

    #[tokio::test]
    async fn missing_command_is_an_error_result() {
        assert!(shell().execute(&json!({}), "").await.is_error);
    }

    #[test]
    fn long_output_keeps_the_tail() {
        let text = format!("{}END", "x".repeat(MAX_OUTPUT_BYTES * 2));
        let cut = truncate(text);
        assert!(cut.ends_with("END"));
        assert!(cut.len() < MAX_OUTPUT_BYTES + 100);
    }
}
