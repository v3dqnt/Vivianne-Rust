//! What the agent loop needs from memory: the tools the model uses (`remember`, `recall`, `reflect`), the small
//! block of standing facts that goes into every prompt, and the note taken when a run that did real work ends.
//!
//! Nothing bulky rides along with every message: recall runs on demand, and only core memory (standing
//! instructions, deliberately saved facts, the user profile) is always in the prompt.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_ai::ai::auth::AuthState;
use vivianne_ai::events::Events;
use vivianne_ai::host::ModelRef;
use vivianne_core::{Message, Role};
use vivianne_tools::{Tool, ToolResult, Toolbox};

use crate::budget;

fn text_result(content: String) -> ToolResult {
    ToolResult {
        is_error: content.starts_with("Error: "),
        content,
        details: None,
    }
}

fn schema_remember() -> Value {
    json!({
        "name": "remember",
        "description": "Save a lasting fact, preference or correction about the user or the project, so it is known in later sessions. Use it for what the user asks you to remember, or states as a standing rule. Do not use it for what only matters in this conversation.",
        "parameters": {
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The fact, written so it makes sense on its own." },
                "key": { "type": "string", "description": "An optional short name for it. Saving again under the same name replaces the older one." }
            },
            "required": ["text"]
        }
    })
}

fn schema_recall() -> Value {
    json!({
        "name": "recall",
        "description": "Search what was remembered in earlier sessions: the user's preferences, decisions, and what happened before. Look here before asking the user something they may already have told you.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "What to look for, in your own words." },
                "as_of": { "type": "string", "description": "Optional: answer as things stood at this date (YYYY-MM-DD)." },
                "depth": { "type": "string", "enum": ["low", "mid", "high"], "description": "How hard to look. Default mid." },
                "max_tokens": { "type": "number", "description": "Most the answer may take." }
            },
            "required": ["query"]
        }
    })
}

fn schema_reflect() -> Value {
    json!({
        "name": "reflect",
        "description": "Answer a question from memory by looking in the right order (the project's pages, then what it has concluded, then raw facts) and saying where the answer came from. Slower than recall; use it for a question that needs putting several memories together.",
        "parameters": {
            "type": "object",
            "properties": { "question": { "type": "string", "description": "The question to answer from memory." } },
            "required": ["question"]
        }
    })
}

/// Everything a memory tool needs: which project's memory, and the services that talk to models.
#[derive(Clone)]
struct Context {
    root: String,
    events: Events,
    model: ModelRef,
}

impl Context {
    async fn auth(&self) -> Arc<AuthState> {
        vivianne_ai::ai::auth::wait_for_state(&self.events).await
    }
}

struct Remember(Context);
struct Recall(Context);
struct Reflect(Context);

#[async_trait]
impl Tool for Remember {
    fn name(&self) -> &str {
        "remember"
    }

    fn definition(&self) -> Option<Value> {
        Some(schema_remember())
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let text = args["text"].as_str().unwrap_or_default();
        let key = args["key"].as_str().map(String::from);
        let auth = self.0.auth().await;
        text_result(
            match crate::remember(&self.0.root, text, key, Some(auth)).await {
                Ok(entry) => format!("Remembered: {}", entry.text),
                Err(e) => format!("Error: {e}"),
            },
        )
    }
}

#[async_trait]
impl Tool for Recall {
    fn name(&self) -> &str {
        "recall"
    }

    fn definition(&self) -> Option<Value> {
        Some(schema_recall())
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let query = args["query"].as_str().unwrap_or_default();
        let auth = self.0.auth().await;
        let as_of = args["as_of"].as_str().and_then(crate::parse_as_of);
        let depth = args["depth"]
            .as_str()
            .and_then(budget::Depth::parse)
            .unwrap_or_default();
        let max_tokens = budget::clamp_tokens(
            args["max_tokens"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok()),
            depth,
        );
        let hits =
            crate::recall_with(&self.0.root, true, query, as_of, Some(&auth), None, depth).await;
        if hits.is_empty() {
            return text_result("No matching memories found.".to_owned());
        }
        // Labelled by source, so the user's own words outrank an inference when two memories disagree. A belief or
        // a page carries its id, which `reflect` can expand into what it rests on.
        let lines: Vec<String> = hits
            .iter()
            .map(|h| {
                let scope = if h.user_wide { ", about you" } else { "" };
                let id = if matches!(h.kind.as_str(), "observation" | "page") {
                    format!(" (id {})", h.id.get(..8).unwrap_or(&h.id))
                } else {
                    String::new()
                };
                format!(
                    "- [{}{scope}] {}{id}",
                    crate::evolve::provenance_label(&h.kind),
                    h.text
                )
            })
            .collect();
        let (shown, left) = budget::pack(&lines, max_tokens);
        let mut out = shown.join("\n");
        if left > 0 {
            out.push_str(&format!("\n({left} more not shown: ask with a larger max_tokens, or use reflect for a worked answer)"));
        }
        text_result(out)
    }
}

#[async_trait]
impl Tool for Reflect {
    fn name(&self) -> &str {
        "reflect"
    }

    fn definition(&self) -> Option<Value> {
        Some(schema_reflect())
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let question = args["question"].as_str().unwrap_or_default().trim();
        if question.is_empty() {
            return text_result("Error: question must not be empty".to_owned());
        }
        text_result(
            match crate::reflect::answer(&self.0.events, &self.0.root, &self.0.model, question)
                .await
            {
                Ok(answer) => answer,
                Err(e) => format!("Error: {e}"),
            },
        )
    }
}

/// Gives the toolbox the memory tools for the project at `root`. `model` is the one `reflect` thinks with.
pub fn add_tools(toolbox: &mut Toolbox, root: &str, events: &Events, model: ModelRef) {
    let context = Context {
        root: root.to_owned(),
        events: events.clone(),
        model,
    };
    toolbox.add(Remember(context.clone()));
    toolbox.add(Recall(context.clone()));
    toolbox.add(Reflect(context));
}

/// A short note on what a run did, when it did real work (changed a file or ran a command): deterministic, with no
/// model call, so pure question-and-answer turns leave nothing behind.
#[must_use]
pub fn turn_summary(new_messages: &[Message]) -> Option<String> {
    let mut touched: Vec<String> = Vec::new();
    let mut commands: Vec<String> = Vec::new();
    let mut did_real_work = false;
    let mut final_text = String::new();
    for message in new_messages.iter().filter(|m| m.role == Role::Assistant) {
        for call in &message.tool_calls {
            match call.name.as_str() {
                "write" | "edit" => {
                    did_real_work = true;
                    if let Some(path) = call.args["path"].as_str()
                        && !touched.iter().any(|t| t == path)
                    {
                        touched.push(path.to_owned());
                    }
                }
                "shell" => {
                    did_real_work = true;
                    if let Some(command) = call.args["command"].as_str() {
                        commands.push(command.chars().take(80).collect());
                    }
                }
                _ => {}
            }
        }
        // The last thing said wins.
        if !message.text.trim().is_empty() {
            final_text.clone_from(&message.text);
        }
    }
    if !did_real_work {
        return None;
    }
    let mut parts = Vec::new();
    if !touched.is_empty() {
        parts.push(format!("Touched: {}", touched.join(", ")));
    }
    if !commands.is_empty() {
        parts.push(format!("Ran: {}", commands.join("; ")));
    }
    if !final_text.trim().is_empty() {
        parts.push(format!(
            "Result: {}",
            final_text.trim().chars().take(400).collect::<String>()
        ));
    }
    (!parts.is_empty()).then(|| parts.join(". "))
}

/// When a run ends: note what it did, and start turning the notes into lasting facts once enough have built up.
/// Does nothing while memory is off for the project.
pub async fn after_run(root: &str, events: &Events, model: &ModelRef, new_messages: &[Message]) {
    let Some(summary) = turn_summary(new_messages) else {
        return;
    };
    let auth = vivianne_ai::ai::auth::wait_for_state(events).await;
    crate::note_model(root, model).await;
    if crate::auto_capture(root, &summary, Some(auth))
        .await
        .is_ok()
    {
        maybe_extract(events, root, model).await;
    }
}

async fn maybe_extract(events: &Events, root: &str, model: &ModelRef) {
    let state = crate::state_snapshot(root).await;
    if !state.enabled || !state.extraction_enabled {
        return;
    }
    let after = state.last_extraction_at.unwrap_or(0);
    let Ok(count) = crate::store::count_since(root, "auto", after).await else {
        return;
    };
    if count < i64::try_from(crate::EXTRACTION_BATCH_SIZE).unwrap_or(i64::MAX) {
        return;
    }
    let (events, root, model) = (events.clone(), root.to_owned(), model.clone());
    tokio::spawn(async move { crate::run_l1_extraction(events, root, model).await });
}

/// The standing facts for this project's prompt, or `None` when memory is off or empty.
pub async fn core_block(root: &str) -> Option<String> {
    crate::evolve::core_block(root).await
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use vivianne_core::ToolCall;

    use super::*;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            args,
        }
    }

    #[test]
    fn a_run_that_only_answered_leaves_no_note_and_one_that_worked_names_what_it_touched() {
        let asked = vec![
            Message::new(Role::User, "what is this?"),
            Message::assistant("a parser", vec![]),
        ];
        assert_eq!(turn_summary(&asked), None);
        let worked = vec![
            Message::assistant(
                "",
                vec![
                    call("edit", json!({"path": "a.rs"})),
                    call("shell", json!({"command": "cargo test"})),
                ],
            ),
            Message::tool_result("c", "ok"),
            Message::assistant("Fixed the parser.", vec![]),
        ];
        let note = turn_summary(&worked).expect("it did real work");
        assert_eq!(
            note,
            "Touched: a.rs. Ran: cargo test. Result: Fixed the parser."
        );
    }

    #[test]
    fn every_tool_carries_its_own_definition_so_the_toolbox_can_offer_it() {
        let mut toolbox = Toolbox::new(std::path::Path::new("."));
        add_tools(&mut toolbox, "/p", &Events::new(), ModelRef::default());
        let names: Vec<String> = toolbox.definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["remember", "recall", "reflect"]);
    }
}
