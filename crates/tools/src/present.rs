//! `present`: the model declares existing files as final deliverables for the user. Nothing is copied: the user opens the
//! current files, and the declaration is the list of absolute paths with optional descriptions. Port of `tool-present`.
//!
//! The declaration is durable because the call and its result are in the saved thread: `vivianne_runtime::deliverables`
//! reads them back.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{Tool, ToolResult};

/// The most files one call may declare.
pub const MAX_FILES: usize = 8;

/// The tool's name.
pub const NAME: &str = "present";

/// Declares files as deliverables.
#[derive(Debug)]
pub struct Present {
    cwd: PathBuf,
}

impl Present {
    #[must_use]
    pub fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }

    /// Checks one declared file and gives its absolute path.
    fn check(&self, path: &str) -> Result<PathBuf, String> {
        if path.trim().is_empty() {
            return Err("present requires a non-empty file path".into());
        }
        let target = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.cwd.join(path)
        };
        // A final symbolic link is refused: the user would open whatever it points at later.
        match std::fs::symlink_metadata(&target) {
            Ok(meta) if !meta.file_type().is_file() => {
                return Err(format!("Cannot present {path}: not a regular file"));
            }
            _ => {}
        }
        match std::fs::metadata(&target) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(format!("Cannot present {path}: not a regular file")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "Cannot present {path}: file not found. Check the path, create the file if needed, and retry."
                ));
            }
            Err(e) => return Err(format!("Cannot present {path}: {e}")),
        }
        let real =
            std::fs::canonicalize(&target).map_err(|e| format!("Cannot present {path}: {e}"))?;
        // Without Windows' verbatim prefix, which no other program prints.
        let text = real.to_string_lossy();
        Ok(text
            .strip_prefix(r"\\?\")
            .filter(|rest| !rest.starts_with("UNC\\"))
            .map_or(real.clone(), PathBuf::from))
    }
}

fn failed(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: true,
        details: None,
    }
}

#[async_trait]
impl Tool for Present {
    fn name(&self) -> &str {
        NAME
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": NAME,
            "description": "Declare existing files as final deliverables for the user. Hosts may show presented files only after your turn ends, so call it once the files are final. If the user needs to review files earlier, such as before a tool call that waits for their answer, link them in your message instead. Use it when the user needs a separate file, especially Office documents, spreadsheets, and slide decks; prefer your final response when that suffices. The user opens the current files; their contents are not copied.",
            "parameters": {
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "description": "Usually the 1-2 most important deliverables; at most 4 per call.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Path of an existing regular file. Relative paths use the working folder." },
                                "description": { "type": "string", "description": "Brief description for the user." }
                            },
                            "required": ["path"]
                        }
                    }
                },
                "required": ["files"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(files) = args.get("files").and_then(Value::as_array) else {
            return failed("present requires a files array");
        };
        if files.is_empty() || files.len() > MAX_FILES {
            return failed(format!("present accepts 1 to {MAX_FILES} files"));
        }
        let mut presented = Vec::new();
        for file in files {
            let Some(path) = file.get("path").and_then(Value::as_str) else {
                return failed("present requires a path for every file");
            };
            let absolute = match self.check(path) {
                Ok(absolute) => absolute,
                Err(message) => return failed(message),
            };
            let mut entry = json!({ "path": absolute.to_string_lossy() });
            if let Some(description) = file.get("description").and_then(Value::as_str) {
                entry["description"] = json!(description);
            }
            presented.push(entry);
        }
        let content = presented
            .iter()
            .filter_map(|f| f["path"].as_str())
            .map(|p| format!("Presented {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        ToolResult {
            content,
            is_error: false,
            details: Some(json!({ "files": presented })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viv-present-{name}-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    fn uuid_like() -> String {
        format!(
            "{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        )
    }

    async fn call(tool: &Present, args: Value) -> ToolResult {
        tool.execute(&args, "").await
    }

    #[tokio::test]
    async fn a_relative_file_is_declared_by_its_absolute_path_with_its_description() {
        let dir = folder("ok");
        std::fs::write(dir.join("report.pdf"), "x").expect("file");
        let out = call(
            &Present::new(&dir),
            json!({ "files": [{ "path": "report.pdf", "description": "the report" }] }),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        assert!(out.content.starts_with("Presented ") && out.content.ends_with("report.pdf"));
        let details = out.details.expect("details");
        let path = details["files"][0]["path"].as_str().expect("path");
        assert!(Path::new(path).is_absolute() && !path.starts_with(r"\\?\"));
        assert_eq!(details["files"][0]["description"], "the report");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn files_outside_the_folder_may_be_presented() {
        let (dir, elsewhere) = (folder("in"), folder("out"));
        std::fs::write(elsewhere.join("deck.pptx"), "x").expect("file");
        let out = call(
            &Present::new(&dir),
            json!({ "files": [{ "path": elsewhere.join("deck.pptx").to_string_lossy() }] }),
        )
        .await;
        assert!(!out.is_error, "{}", out.content);
        let _ = (
            std::fs::remove_dir_all(&dir),
            std::fs::remove_dir_all(&elsewhere),
        );
    }

    #[tokio::test]
    async fn missing_files_folders_and_bad_calls_fail_with_a_reason_the_model_can_act_on() {
        let dir = folder("bad");
        std::fs::create_dir_all(dir.join("sub")).expect("sub");
        let tool = Present::new(&dir);
        let missing = call(&tool, json!({ "files": [{ "path": "nope.txt" }] })).await;
        assert!(
            missing.is_error
                && missing.content.contains("file not found")
                && missing.content.contains("retry")
        );
        assert!(
            call(&tool, json!({ "files": [{ "path": "sub" }] }))
                .await
                .content
                .contains("not a regular file")
        );
        assert!(
            call(&tool, json!({ "files": [{ "path": "  " }] }))
                .await
                .content
                .contains("non-empty")
        );
        assert!(
            call(&tool, json!({ "files": [] }))
                .await
                .content
                .contains("1 to 8")
        );
        let many: Vec<Value> = (0..9).map(|i| json!({ "path": format!("f{i}") })).collect();
        assert!(call(&tool, json!({ "files": many })).await.is_error);
        assert!(call(&tool, json!({})).await.is_error);
        assert!(
            call(&tool, json!({ "files": [{ "description": "no path" }] }))
                .await
                .is_error
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn one_bad_file_declares_none_of_the_call() {
        let dir = folder("all-or-nothing");
        std::fs::write(dir.join("good.txt"), "x").expect("file");
        let out = call(
            &Present::new(&dir),
            json!({ "files": [{ "path": "good.txt" }, { "path": "bad.txt" }] }),
        )
        .await;
        assert!(out.is_error && out.details.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_final_symbolic_link_is_refused() {
        let dir = folder("link");
        std::fs::write(dir.join("real.txt"), "x").expect("file");
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("link.txt")).expect("link");
        assert!(
            call(
                &Present::new(&dir),
                json!({ "files": [{ "path": "link.txt" }] })
            )
            .await
            .is_error
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_schema_names_the_tool_and_requires_the_files() {
        let definition = Present::new(Path::new("."))
            .definition()
            .expect("definition");
        assert_eq!(definition["name"], NAME);
        assert_eq!(definition["parameters"]["required"][0], "files");
    }
}
