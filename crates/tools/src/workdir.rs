//! The conversation's working directory: where relative paths and commands start. It begins at the project's folder, the
//! agent can move it with the `working_directory` tool, and it can never leave the project: a target must be an existing
//! folder that, with links resolved, lies inside the project's root. Port of the original `working-directory` and
//! `tool-working-directory`.
//!
//! Tools keep the folder they were built with, so the toolbox does the translating: before a call it makes the call's
//! relative paths absolute from the current directory (and gives a command the directory to run in), so no tool has to
//! know the directory can move. The same state is shared with programs run by `run_code`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{Tool, ToolResult};

/// The tool's name.
pub const NAME: &str = "working_directory";

/// Arguments that name a path, in the tools that take one.
const PATH_ARGUMENTS: [&str; 5] = ["path", "file_path", "filePath", "output", "directory"];

/// Tools whose `path` falls back to the working directory when left out, so a moved directory changes what they search.
const DEFAULT_TO_DIRECTORY: [&str; 4] = ["ls", "grep", "code_search", "glob"];

/// A path without Windows' `\\?\` verbatim prefix.
fn plain(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

/// The current directory, shared by the toolbox and the tools that need to read or change it.
#[derive(Debug, Clone)]
pub struct WorkingDirectory {
    root: PathBuf,
    current: Arc<Mutex<PathBuf>>,
}

impl WorkingDirectory {
    /// Starts at `root`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        Self { root: root.to_path_buf(), current: Arc::new(Mutex::new(root.to_path_buf())) }
    }

    /// The project's folder, which the directory can never leave.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where relative paths start now.
    #[must_use]
    pub fn get(&self) -> PathBuf {
        self.current.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Whether it has moved from the project's folder.
    #[must_use]
    pub fn moved(&self) -> bool {
        self.get() != self.root
    }

    /// Goes back to the project's folder.
    pub fn reset(&self) {
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = self.root.clone();
    }

    /// Enters `path` (relative to the current directory, or absolute) if it is an existing folder inside the project.
    /// Returns the new directory; on a refusal nothing changes.
    pub fn enter(&self, path: &str) -> Result<PathBuf, String> {
        if path.trim().is_empty() {
            return Err("working_directory needs a non-empty path".into());
        }
        let wanted = if Path::new(path).is_absolute() { PathBuf::from(path) } else { self.get().join(path) };
        let real = std::fs::canonicalize(&wanted).map_err(|_| format!("{path} is not an existing directory"))?;
        let real_root = std::fs::canonicalize(&self.root).map_err(|e| e.to_string())?;
        if !real.is_dir() {
            return Err(format!("{path} is not a directory"));
        }
        // Links are resolved before the comparison, so a link inside the project that points out of it is refused.
        if !real.starts_with(&real_root) {
            return Err(format!("{path} is outside the project, which the working directory cannot leave"));
        }
        // Kept in the spelling the project uses, so paths printed later match the ones the model has seen.
        let shown = match real.strip_prefix(&real_root) {
            Ok(inside) => self.root.join(inside),
            Err(_) => plain(real),
        };
        *self.current.lock().unwrap_or_else(PoisonError::into_inner) = shown.clone();
        Ok(shown)
    }

    /// The call with its relative paths made absolute from the current directory. Nothing changes while the directory is
    /// still the project's folder, so tools behave exactly as before until it is moved.
    #[must_use]
    pub fn localize(&self, tool: &str, args: &Value) -> Value {
        if !self.moved() {
            return args.clone();
        }
        let here = self.get();
        let mut out = args.clone();
        let Some(map) = out.as_object_mut() else { return out };
        for key in PATH_ARGUMENTS {
            if let Some(Value::String(path)) = map.get_mut(key)
                && !path.is_empty()
                && Path::new(path.as_str()).is_relative()
            {
                *path = here.join(path.as_str()).to_string_lossy().into_owned();
            }
        }
        if DEFAULT_TO_DIRECTORY.contains(&tool) && map.get("path").is_none_or(Value::is_null) {
            map.insert("path".to_owned(), Value::String(here.to_string_lossy().into_owned()));
        }
        // The files of a `present` call.
        if let Some(Value::Array(files)) = map.get_mut("files") {
            for file in files {
                if let Some(Value::String(path)) = file.get_mut("path")
                    && !path.is_empty()
                    && Path::new(path.as_str()).is_relative()
                {
                    *path = here.join(path.as_str()).to_string_lossy().into_owned();
                }
            }
        }
        if tool == "shell" {
            map.entry("cwd".to_owned()).or_insert_with(|| Value::String(here.to_string_lossy().into_owned()));
        }
        out
    }

    /// The directory the conversation ended in, read back from its history: the last successful `working_directory` call
    /// that named one, replayed from the project's folder so each step is checked as it was. Failed calls changed nothing.
    pub fn replay(&self, history: &[(String, Value, bool)]) {
        self.reset();
        for (name, args, ok) in history {
            if name == NAME
                && *ok
                && let Some(path) = args.get("path").and_then(Value::as_str)
            {
                let _ = self.enter(path);
            }
        }
    }
}

/// Reads or changes the working directory.
#[derive(Debug)]
pub struct WorkingDirectoryTool {
    directory: WorkingDirectory,
}

impl WorkingDirectoryTool {
    #[must_use]
    pub fn new(directory: WorkingDirectory) -> Self {
        Self { directory }
    }
}

#[async_trait]
impl Tool for WorkingDirectoryTool {
    fn name(&self) -> &str {
        NAME
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": NAME,
            "description": "Read the current working directory, or move it to an existing directory inside the project. Relative paths in other tools and the shell start from it. Use it with no path to see where you are.",
            "parameters": {
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Directory to enter: relative to the current one, or absolute. Omit to read the current directory." } },
                "required": []
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        match args.get("path").and_then(Value::as_str) {
            None => ToolResult::ok(format!("Working directory: {}", self.directory.get().display())),
            Some(path) => match self.directory.enter(path) {
                Ok(now) => ToolResult::ok(format!("Working directory: {}", now.display())).with_details(json!({ "path": now.to_string_lossy() })),
                Err(why) => ToolResult::error(why),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (PathBuf, WorkingDirectory) {
        let root = crate::testing::scratch("workdir");
        std::fs::create_dir_all(root.join("a/b")).expect("dirs");
        std::fs::write(root.join("a/file.txt"), "x").expect("file");
        let root = std::fs::canonicalize(&root).expect("canonical");
        let root = plain(root);
        (root.clone(), WorkingDirectory::new(&root))
    }

    #[test]
    fn it_starts_at_the_project_and_enters_folders_relative_to_where_it_is() {
        let (root, wd) = project();
        assert_eq!((wd.get(), wd.moved()), (root.clone(), false));
        assert_eq!(wd.enter("a").expect("enters"), root.join("a"));
        assert_eq!(wd.enter("b").expect("relative to the current one"), root.join("a").join("b"));
        assert_eq!(wd.enter("..").expect("up one"), root.join("a"));
        assert!(wd.moved());
        wd.reset();
        assert_eq!(wd.get(), root);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn it_never_leaves_the_project_and_a_refusal_changes_nothing() {
        let (root, wd) = project();
        wd.enter("a").expect("enters");
        for bad in ["../..", "../../..", "nope", "file.txt", "", "  "] {
            assert!(wd.enter(bad).is_err(), "{bad:?}");
        }
        assert!(wd.enter(&std::env::temp_dir().to_string_lossy()).is_err() || std::env::temp_dir().starts_with(&root));
        assert_eq!(wd.get(), root.join("a"), "unchanged");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_project_that_points_out_of_it_is_refused() {
        let (root, wd) = project();
        let outside = crate::testing::scratch("workdir-outside");
        std::os::unix::fs::symlink(&outside, root.join("escape")).expect("link");
        assert!(wd.enter("escape").unwrap_err().contains("outside the project"));
        let _ = (std::fs::remove_dir_all(&root), std::fs::remove_dir_all(&outside));
    }

    #[test]
    fn calls_are_left_alone_until_the_directory_moves_and_then_start_from_it() {
        let (root, wd) = project();
        let read = json!({ "path": "file.txt", "limit": 3 });
        assert_eq!(wd.localize("read", &read), read, "not moved: untouched");
        wd.enter("a").expect("enters");
        let here = root.join("a");
        assert_eq!(wd.localize("read", &read)["path"], here.join("file.txt").to_string_lossy().as_ref());
        assert_eq!(wd.localize("read", &read)["limit"], 3, "other arguments are kept");
        let absolute = json!({ "path": root.join("x.txt").to_string_lossy() });
        assert_eq!(wd.localize("read", &absolute), absolute, "an absolute path stays as it is");
        assert_eq!(wd.localize("grep", &json!({ "pattern": "x" }))["path"], here.to_string_lossy().as_ref(), "a search with no path searches here");
        assert_eq!(wd.localize("read", &json!({ "offset": 1 })).get("path"), None, "a tool that needs a path is not given one");
        assert_eq!(wd.localize("shell", &json!({ "command": "ls" }))["cwd"], here.to_string_lossy().as_ref());
        let files = wd.localize("present", &json!({ "files": [{ "path": "file.txt", "description": "d" }] }));
        assert_eq!(files["files"][0]["path"], here.join("file.txt").to_string_lossy().as_ref());
        assert_eq!(wd.localize("convert_to_pdf", &json!({ "path": "r.docx", "output": "out/r.pdf" }))["output"], here.join("out/r.pdf").to_string_lossy().as_ref());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_directory_is_read_back_from_a_conversation_and_failed_calls_change_nothing() {
        let (root, wd) = project();
        let call = |path: &str, ok: bool| (NAME.to_owned(), json!({ "path": path }), ok);
        wd.replay(&[call("a", true), call("../..", false), call("b", true), ("read".to_owned(), json!({ "path": "x" }), true)]);
        assert_eq!(wd.get(), root.join("a").join("b"));
        wd.replay(&[]);
        assert_eq!(wd.get(), root, "an empty conversation starts at the project");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_toolbox_follows_the_directory_for_reads_searches_commands_and_programs() {
        let (root, _) = project();
        std::fs::write(root.join("a/inner.txt"), "inside a\n").expect("file");
        std::fs::write(root.join("inner.txt"), "at the top\n").expect("file");
        let toolbox = crate::Toolbox::coding(&root);
        let read = |path: &'static str| {
            let toolbox = &toolbox;
            async move { toolbox.execute("read", &json!({ "path": path })).await.expect("read exists").content }
        };
        assert!(read("inner.txt").await.contains("at the top"));
        let moved = toolbox.execute(NAME, &json!({ "path": "a" })).await.expect("tool exists");
        assert!(!moved.is_error, "{}", moved.content);
        assert!(read("inner.txt").await.contains("inside a"), "a relative path now starts in a/");
        let listed = toolbox.execute("ls", &json!({})).await.expect("ls exists").content;
        assert!(listed.contains("inner.txt") && listed.contains("file.txt"), "{listed}");
        let shell = toolbox.execute("shell", &json!({ "command": if cfg!(windows) { "Get-Content inner.txt" } else { "cat inner.txt" } })).await.expect("shell exists");
        assert!(shell.content.contains("inside a"), "a command starts in the working directory: {}", shell.content);
        let program = toolbox.execute("run_code", &json!({ "code": "return await tools.read({ path: 'inner.txt' });" })).await.expect("run_code exists");
        assert!(program.content.contains("inside a"), "a program starts there too: {}", program.content);
        let _ = toolbox.execute(NAME, &json!({ "path": root.to_string_lossy() })).await;
        assert!(read("inner.txt").await.contains("at the top"), "back at the project");
        let refused = toolbox.execute(NAME, &json!({ "path": ".." })).await.expect("tool exists");
        assert!(refused.is_error && refused.content.contains("outside the project"), "{}", refused.content);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_tool_reads_and_changes_the_directory_and_says_why_when_it_cannot() {
        let (root, wd) = project();
        let tool = WorkingDirectoryTool::new(wd.clone());
        let read = tool.execute(&json!({}), "").await;
        assert!(!read.is_error && read.content.contains(&root.to_string_lossy().to_string()), "{}", read.content);
        let moved = tool.execute(&json!({ "path": "a" }), "").await;
        assert!(!moved.is_error && moved.content.ends_with(&root.join("a").to_string_lossy().to_string()), "{}", moved.content);
        let refused = tool.execute(&json!({ "path": "../.." }), "").await;
        assert!(refused.is_error && refused.content.contains("outside the project"), "{}", refused.content);
        assert_eq!(wd.get(), root.join("a"));
        assert_eq!(tool.definition().expect("definition")["name"], NAME);
        let _ = std::fs::remove_dir_all(&root);
    }
}
