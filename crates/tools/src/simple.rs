#![allow(clippy::pedantic, missing_debug_implementations)] // stand-ins: mirror the original signatures
//! The stand-in `shell`, `write` and `edit` tools (run a command with a timeout, atomic write, exact
//! single-match edit), confined to the project folder (`Workspace`). Replaced by the original tools
//! (`bash`, `powershell`, ...) once those are copied.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_sandbox::policy::{self, Mode, Policy};
use vivianne_sandbox::confine;

use crate::builtin::{Tool, ToolResult};
use crate::confinement::Confinement;
use crate::jobs::Jobs;

mod background;

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
                    "timeout_secs": {"type": "integer", "description": "default 120, max 600. On expiry the command moves to the background as a job instead of being killed."},
                    "run_in_background": {"type": "boolean", "description": "Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies."}
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

// ── The folder the tools may touch ───────────────────────────────────────

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
mod workspace_tests {
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

// ── shell ────────────────────────────────────────────────────────────────

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub struct Shell {
    workspace: Workspace,
    confinement: Confinement,
    jobs: Option<Jobs>,
}

impl Shell {
    #[must_use]
    pub fn new(workspace: Workspace) -> Self {
        Self { workspace, confinement: Confinement::default(), jobs: None }
    }

    #[must_use]
    pub fn confined(mut self, confinement: Confinement) -> Self {
        self.confinement = confinement;
        self
    }

    /// Lets commands run as background jobs, and move there when they outlast their timeout.
    #[must_use]
    pub fn with_jobs(mut self, jobs: Jobs) -> Self {
        self.jobs = Some(jobs);
        self
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
        // Where the command starts: the working directory the toolbox gave it, if that is inside the project.
        let start = args["cwd"].as_str().and_then(|dir| self.workspace.resolve(dir).ok()).filter(|dir| dir.is_dir());
        let start = start.unwrap_or_else(|| self.workspace.root().to_path_buf());
        // A call that asked to go wider was approved by the person before it reached here.
        let standing = self.confinement.get();
        let mode = self.confinement.asked("shell", args).map_err(ToolError::BadArgs)?.unwrap_or(standing);
        let argv = self.argv(command, mode)?;
        let call = background::Command {
            command,
            argv: argv.argv,
            start,
            mode,
            denial_signatures: argv.denial_signatures,
            failure_signatures: argv.failure_signatures,
        };
        let limit = Duration::from_secs(secs);
        let background = args["run_in_background"].as_bool() == Some(true);
        match &self.jobs {
            Some(jobs) => background::run(jobs, call, limit, background).await,
            None if background => Err(ToolError::Failed("background jobs are unavailable here".to_owned())),
            None => background::foreground(call, limit).await,
        }
    }
}

impl Shell {
    /// The platform's shell running `command`, wrapped for `mode`. A mode this machine cannot enforce is refused.
    fn argv(&self, command: &str, mode: Mode) -> Result<ShellArgv, ToolError> {
        let plain: Vec<String> = if cfg!(windows) {
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", command].map(str::to_owned).to_vec()
        } else {
            ["sh", "-c", command].map(str::to_owned).to_vec()
        };
        if mode == Mode::DangerFullAccess {
            return Ok(ShellArgv { argv: plain, denial_signatures: &[], failure_signatures: &[] });
        }
        let root = std::path::absolute(self.workspace.root()).map_err(failed)?;
        let wanted = Policy::new(mode, &root, &[]).map_err(failed)?;
        let confined = confine::confine(&plain, &wanted).map_err(failed)?;
        Ok(ShellArgv { argv: confined.argv, denial_signatures: confined.denial_signatures, failure_signatures: confined.failure_signatures })
    }
}

struct ShellArgv {
    argv: Vec<String>,
    denial_signatures: &'static [&'static str],
    failure_signatures: &'static [&'static str],
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
mod shell_tests {
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

// ── write and edit ───────────────────────────────────────────────────────

/// Under `read-only` no file is changed. Writes are already kept inside the project, which is what
/// `workspace-write` allows.
fn writable(confinement: &Confinement) -> Result<(), ToolError> {
    if confinement.get() == Mode::ReadOnly {
        return Err(ToolError::Failed(format!("{} The user can change the sandbox setting.", policy::denial_marker(Mode::ReadOnly))));
    }
    Ok(())
}

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
pub struct WriteFile(Workspace, Confinement);

impl WriteFile {
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        Self(ws, Confinement::default())
    }

    #[must_use]
    pub fn confined(mut self, confinement: Confinement) -> Self {
        self.1 = confinement;
        self
    }
}

#[async_trait]
impl SimpleTool for WriteFile {
    fn name(&self) -> &str {
        "write"
    }

    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError> {
        writable(&self.1)?;
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
pub struct EditFile(Workspace, Confinement);

impl EditFile {
    #[must_use]
    pub fn new(ws: Workspace) -> Self {
        Self(ws, Confinement::default())
    }

    #[must_use]
    pub fn confined(mut self, confinement: Confinement) -> Self {
        self.1 = confinement;
        self
    }
}

#[async_trait]
impl SimpleTool for EditFile {
    fn name(&self) -> &str {
        "edit"
    }

    async fn call(&self, args: &Value) -> Result<SimpleOutput, ToolError> {
        writable(&self.1)?;
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
mod file_tests {
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
        let out = WriteFile::new(ws.clone())
            .execute(&json!({"path": "a/b.txt", "content": "one two"}), "")
            .await;
        assert!(!out.is_error, "{}", out.content);
        let edit = json!({"path": "a/b.txt", "old_string": "two", "new_string": "2"});
        assert!(!EditFile::new(ws.clone()).execute(&edit, "").await.is_error);
        let text = std::fs::read_to_string(ws.root().join("a").join("b.txt")).unwrap();
        assert_eq!(text, "one 2");
        let _ = std::fs::remove_dir_all(ws.root());
    }

    #[tokio::test]
    async fn edit_refuses_ambiguous_and_missing_matches() {
        let ws = scratch("edit");
        WriteFile::new(ws.clone())
            .execute(&json!({"path": "f", "content": "aa"}), "")
            .await;
        let edit = EditFile::new(ws.clone());
        let ambiguous = json!({"path": "f", "old_string": "a", "new_string": "b"});
        let missing = json!({"path": "f", "old_string": "z", "new_string": "b"});
        assert!(edit.execute(&ambiguous, "").await.is_error);
        assert!(edit.execute(&missing, "").await.is_error);
        let _ = std::fs::remove_dir_all(ws.root());
    }

    #[tokio::test]
    async fn writing_outside_the_workspace_is_refused() {
        let ws = scratch("fence");
        let out = WriteFile::new(ws)
            .execute(&json!({"path": "../../x", "content": "no"}), "")
            .await;
        assert!(out.is_error);
    }
}

#[cfg(test)]
mod confined_tests {
    use super::*;
    use crate::builtin::Tool;

    #[tokio::test]
    async fn read_only_refuses_writes_and_a_command_that_cannot_be_confined_does_not_run() {
        let dir = std::env::temp_dir().join(format!("viv-confined-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let confinement = Confinement::default();
        confinement.set(Mode::ReadOnly);
        let write = WriteFile::new(Workspace::new(&dir)).confined(confinement.clone());
        let refused = write.execute(&json!({"path": "a.txt", "content": "x"}), "").await;
        assert!(refused.is_error && refused.content.contains("read-only"), "{}", refused.content);
        assert!(!dir.join("a.txt").exists());
        if cfg!(windows) {
            let shell = Shell::new(Workspace::new(&dir)).confined(confinement);
            let out = shell.execute(&json!({"command": "Set-Content b.txt x"}), "").await;
            assert!(out.is_error && out.content.contains("refusing to run the command unconfined"), "{}", out.content);
            assert!(!dir.join("b.txt").exists());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
