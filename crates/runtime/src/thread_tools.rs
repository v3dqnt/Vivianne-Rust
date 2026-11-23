//! Tools that let an agent look at the user's other conversations in the same project: list them, and read one.
//! Offered to coding agents (see the shell's `own_tools`), so work started in one thread can be picked up in
//! another. Read-only, and limited to the project the call is made in.

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::Role;
use vivianne_tools::{Tool, ToolResult};

use crate::store;

/// Most characters of a conversation one read puts into an agent's context.
const MAX_READ_CHARS: usize = 40_000;

/// Lists the conversations started in a project folder, newest first.
#[derive(Debug)]
pub struct ListThreads {
    workspace: String,
}

/// Reads one of them.
#[derive(Debug)]
pub struct ReadThread {
    workspace: String,
}

impl ListThreads {
    #[must_use]
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
        }
    }
}

impl ReadThread {
    #[must_use]
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
        }
    }
}

fn ok(content: String) -> ToolResult {
    ToolResult {
        content,
        is_error: false,
        details: None,
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
impl Tool for ListThreads {
    fn name(&self) -> &str {
        "list_threads"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "list_threads",
            "description": "List the user's other conversations in this project, newest first, each with its id and title. Read one with read_thread.",
            "parameters": { "type": "object", "properties": {} }
        }))
    }

    async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
        let threads = store::list(&self.workspace);
        if threads.is_empty() {
            return ok("There are no other saved conversations in this project.".to_owned());
        }
        ok(threads
            .iter()
            .map(|t| format!("- {} (id {})", t.title, t.id))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[async_trait]
impl Tool for ReadThread {
    fn name(&self) -> &str {
        "read_thread"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "read_thread",
            "description": "Read one of the user's other conversations in this project: what was said, and which tools were used. Use an id from list_threads.",
            "parameters": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The conversation's id." } },
                "required": ["id"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let id = args["id"].as_str().unwrap_or_default();
        // Only this project's conversations: an id from another project reads as if there were none.
        let Some(thread) = store::load(id).filter(|t| t.workspace == self.workspace) else {
            return failed(format!(
                "There is no saved conversation {id} in this project."
            ));
        };
        let mut out = format!("# {}\n\n", thread.title);
        for message in &thread.history {
            match message.role {
                Role::User => out.push_str(&format!("User: {}\n\n", message.text.trim())),
                Role::Assistant => {
                    if !message.text.trim().is_empty() {
                        out.push_str(&format!("Assistant: {}\n\n", message.text.trim()));
                    }
                    for call in &message.tool_calls {
                        out.push_str(&format!("(used {})\n\n", call.name));
                    }
                }
                Role::Tool | Role::System => {}
            }
        }
        // The newest part is what carries on from where it was left.
        let skip = out.chars().count().saturating_sub(MAX_READ_CHARS);
        ok(if skip > 0 {
            format!(
                "(the earlier part is left out)\n{}",
                out.chars().skip(skip).collect::<String>()
            )
        } else {
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use vivianne_core::{Message, Mode};

    use super::*;

    #[tokio::test]
    async fn the_threads_of_a_project_are_listed_and_read_and_another_projects_are_not() {
        let workspace = format!("/viv-test-{}", uuid::Uuid::new_v4());
        let id = uuid::Uuid::new_v4().to_string();
        let thread = |id: &str, workspace: &str| store::SavedThread {
            id: id.to_owned(),
            workspace: workspace.to_owned(),
            title: "Fix the parser".into(),
            title_source: "auto".into(),
            provider: "p".into(),
            model: "m".into(),
            mode: Mode::Ask,
            updated_at: store::now(),
            history: vec![
                Message::new(Role::User, "the parser panics"),
                Message::assistant("Fixed it.", vec![]),
            ],
        };
        let elsewhere = uuid::Uuid::new_v4().to_string();
        store::save(&thread(&id, &workspace)).expect("saves");
        store::save(&thread(&elsewhere, "/another-project")).expect("saves");

        let listed = ListThreads::new(&workspace).execute(&json!({}), ".").await;
        assert!(
            listed.content.contains(&id) && !listed.content.contains(&elsewhere),
            "{}",
            listed.content
        );
        let read = ReadThread::new(&workspace)
            .execute(&json!({ "id": id }), ".")
            .await;
        assert!(
            !read.is_error
                && read.content.contains("User: the parser panics")
                && read.content.contains("Assistant: Fixed it.")
        );
        let other = ReadThread::new(&workspace)
            .execute(&json!({ "id": elsewhere }), ".")
            .await;
        assert!(
            other.is_error,
            "another project's conversation is not readable from here"
        );
        let escape = ReadThread::new(&workspace)
            .execute(&json!({ "id": "../acp-sessions" }), ".")
            .await;
        assert!(escape.is_error, "an id is never a path");

        let _ = store::delete(&id);
        let _ = store::delete(&elsewhere);
    }
}
