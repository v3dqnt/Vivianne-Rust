//! The tools an agent uses to manage automations: `create_automation`, `list_automations`, `delete_automation` and
//! `set_automation_enabled`.

use async_trait::async_trait;
use chrono::{DateTime, Local};
use serde_json::{Value, json};
use vivianne_tools::{Tool, ToolResult};

use super::{New, Step};

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// An epoch time as the user's own clock reads it.
fn when(ms: Option<u64>) -> String {
    ms.and_then(|ms| DateTime::from_timestamp_millis(i64::try_from(ms).ok()?))
        .map_or_else(
            || "never".to_owned(),
            |t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string(),
        )
}

#[derive(Debug, Default)]
pub struct CreateAutomation;

#[async_trait]
impl Tool for CreateAutomation {
    fn name(&self) -> &str {
        "create_automation"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "create_automation",
            "description": "Have a prompt run through the agent by itself: on a cron schedule, or whenever a file appears or changes in a folder. Created enabled; the user can see and switch it off in Settings. For a schedule given in words ('every morning at 9'), work out the cron expression from the current time, and ask the user when they have not said.",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "A short name." },
                    "prompt": { "type": "string", "description": "What is sent to the agent each time it fires." },
                    "cron": { "type": "string", "description": "Five fields, `min hour day-of-month month day-of-week`, in the user's time: `0 9 * * *` is nine every day, `*/15 * * * *` every fifteen minutes, `0 9 * * 1` nine on Mondays. Not needed with a folder." },
                    "folder": { "type": "string", "description": "Instead of a schedule: fire when a file is new or changed in this folder. The file's path is added to the prompt." },
                    "glob": { "type": "string", "description": "With a folder: only file names that match (`*` and `?`)." },
                    "steps": { "type": "array", "items": { "type": "object", "properties": { "prompt": { "type": "string" }, "ask_first": { "type": "boolean" } }, "required": ["prompt"] }, "description": "More prompts to run after the first, in order, in the same conversation. ask_first stops and asks the user before that step." }
                },
                "required": ["name", "prompt"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let steps = args
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .map(|s| Step {
                        prompt: text(s, "prompt").unwrap_or_default().to_owned(),
                        ask_first: s.get("ask_first").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let new = New {
            name: text(args, "name").unwrap_or_default().to_owned(),
            prompt: text(args, "prompt").unwrap_or_default().to_owned(),
            cron: text(args, "cron").unwrap_or_default().to_owned(),
            steps,
            folder: text(args, "folder").map(str::to_owned),
            glob: text(args, "glob").map(str::to_owned),
        };
        match super::create(new) {
            Ok(a) if a.folder.is_some() => ToolResult::ok(format!(
                "Created automation '{}' (id: {}). It fires when a file is new or changed in {}.",
                a.name,
                a.id,
                a.folder.unwrap_or_default()
            )),
            Ok(a) => ToolResult::ok(format!(
                "Created automation '{}' (id: {}). Next run: {}. It is enabled and shown in Settings.",
                a.name,
                a.id,
                when(a.next_run)
            )),
            Err(why) => ToolResult::failure(why),
        }
    }
}

#[derive(Debug, Default)]
pub struct ListAutomations;

#[async_trait]
impl Tool for ListAutomations {
    fn name(&self) -> &str {
        "list_automations"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "list_automations",
            "description": "List the automations: name, id, schedule or folder, whether on, when last and next run.",
            "parameters": { "type": "object", "properties": {} }
        }))
    }

    async fn execute(&self, _args: &Value, _cwd: &str) -> ToolResult {
        let items = super::list();
        if items.is_empty() {
            return ToolResult::ok("No automations.");
        }
        ToolResult::ok(
            items
                .iter()
                .map(|a| {
                    let trigger = a.folder.as_ref().map_or_else(
                        || format!("cron `{}`, next {}", a.cron, when(a.next_run)),
                        |f| format!("folder {f}"),
                    );
                    format!(
                        "- \"{}\" (id: {}) {}; {}; last run {}",
                        a.name,
                        a.id,
                        if a.enabled { "on" } else { "off" },
                        trigger,
                        when(a.last_run)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

#[derive(Debug, Default)]
pub struct DeleteAutomation;

#[async_trait]
impl Tool for DeleteAutomation {
    fn name(&self) -> &str {
        "delete_automation"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "delete_automation",
            "description": "Delete an automation for good (ids from list_automations).",
            "parameters": { "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let Some(id) = text(args, "id") else {
            return ToolResult::failure("'id' is required.");
        };
        if super::delete(id) {
            ToolResult::ok(format!("Deleted automation {id}."))
        } else {
            ToolResult::failure(format!("no automation with id {id}."))
        }
    }
}

#[derive(Debug, Default)]
pub struct SetAutomationEnabled;

#[async_trait]
impl Tool for SetAutomationEnabled {
    fn name(&self) -> &str {
        "set_automation_enabled"
    }

    fn definition(&self) -> Option<Value> {
        Some(json!({
            "name": "set_automation_enabled",
            "description": "Turn an automation on or off without deleting it.",
            "parameters": {
                "type": "object",
                "properties": { "id": { "type": "string" }, "enabled": { "type": "boolean" } },
                "required": ["id", "enabled"]
            }
        }))
    }

    async fn execute(&self, args: &Value, _cwd: &str) -> ToolResult {
        let (Some(id), Some(enabled)) = (
            text(args, "id"),
            args.get("enabled").and_then(Value::as_bool),
        ) else {
            return ToolResult::failure("'id' and 'enabled' are required.");
        };
        match super::set_enabled(id, enabled) {
            Ok(a) => ToolResult::ok(format!(
                "Automation '{}' is now {}.",
                a.name,
                if a.enabled { "on" } else { "off" }
            )),
            Err(why) => ToolResult::failure(why),
        }
    }
}
