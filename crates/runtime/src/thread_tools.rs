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
            return ToolResult::ok(
                "There are no other saved conversations in this project.".to_owned(),
            );
        }
        ToolResult::ok(
            threads
                .iter()
                .map(|t| format!("- {} (id {})", t.title, t.id))
                .collect::<Vec<_>>()
                .join("\n"),
        )
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
            return ToolResult::error(format!(
                "There is no saved conversation {id} in this project."
            ));
        };
        let mut out = format!("# {}\n\n", thread.title);
        for message in &thread.history {
            match message.role {
                // What the app added (instructions, the clock) is not part of the conversation being read.
                Role::User if message.context.is_some() => {}
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
        ToolResult::ok(if skip > 0 {
            format!(
                "(the earlier part is left out)\n{}",
                out.chars().skip(skip).collect::<String>()
            )
        } else {
            out
        })
    }
}

/// Searches the words of this project's conversations.
#[derive(Debug)]
pub struct SearchThreads {
    workspace: String,
}

impl SearchThreads {
    #[must_use]
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
        }
    }
}

/// Brings the index up to date and searches it. Blocking work.
fn search(workspace: String, words: String, limit: usize) -> Result<Value, String> {
    use crate::query::Query;
    use crate::query::filters::ThreadFilter;
    use crate::query::search::{Index, Search};
    let query = Query::standard().map_err(|e| e.to_string())?;
    let mut index = Index::open(&vivianne_ai::paths::viv_agent_dir()?.join("search.db")).map_err(|e| e.to_string())?;
    index.reconcile(&query).map_err(|e| e.to_string())?;
    let request = Search {
        query: words,
        threads: vec![ThreadFilter::Workspace(vec![workspace])],
        limit: Some(limit),
        ..Search::default()
    };
    let page = index.search_threads(&request).map_err(|e| e.to_string())?;
    Ok(json!({ "hits": page.items.iter().map(|h| json!({ "thread": h.thread, "position": h.best.record.seq, "snippet": h.best.snippet })).collect::<Vec<_>>() }))
}

#[async_trait]
impl Tool for SearchThreads {
    fn name(&self) -> &str {
        "search_threads"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "search_threads",
            "description": "Search the words said in the user's other conversations in this project. Words and \"quoted phrases\", all required. Read a found conversation with read_thread.",
            "parameters": { "type": "object", "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "description": "default 10, at most 50" }
            }, "required": ["query"] }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let words = args["query"].as_str().unwrap_or_default().to_owned();
        let limit = args["limit"].as_u64().and_then(|n| usize::try_from(n).ok()).unwrap_or(10).min(50);
        let workspace = self.workspace.clone();
        match tokio::task::spawn_blocking(move || search(workspace, words, limit)).await {
            Ok(Ok(found)) => ToolResult::ok(found.to_string()),
            Ok(Err(why)) => ToolResult::error(why),
            Err(why) => ToolResult::error(why.to_string()),
        }
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
            parent: None,
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
