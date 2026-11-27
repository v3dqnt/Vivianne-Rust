//! What the agent loop does, said in the protocol's words.
//!
//! Port of `@deepseek-ai/dsh-acp/updates.ts` and `codec.ts`, over this loop's events.

use serde_json::{Value, json};
use vivianne_runtime::AgentEvent;

/// The tool kind a client draws an icon for.
fn kind(tool: &str) -> &'static str {
    match tool {
        "read" | "ls" => "read",
        "write" | "edit" => "edit",
        "grep" | "glob" | "code_search" => "search",
        "shell" | "terminal_send" => "execute",
        "repo_clone" => "fetch",
        _ => "other",
    }
}

fn text_block(text: &str) -> Value {
    json!({ "type": "text", "text": text })
}

/// The `session/update` for one event, if it is one a client is told about.
pub(super) fn update(event: &AgentEvent) -> Option<Value> {
    match event {
        AgentEvent::Text(text) => Some(json!({
            "sessionUpdate": "agent_message_chunk",
            "content": text_block(text),
        })),
        AgentEvent::ToolStart { id, name, args } => Some(json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": name,
            "kind": kind(name),
            "status": "in_progress",
            "rawInput": args,
        })),
        AgentEvent::ToolEnd {
            id,
            output,
            is_error,
            ..
        } => Some(json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": if *is_error { "failed" } else { "completed" },
            "content": [{ "type": "content", "content": text_block(output) }],
        })),
        _ => None,
    }
}

/// Why a turn ended, from the last thing the loop said about itself.
pub(super) fn stop_reason(last: Option<&AgentEvent>) -> &'static str {
    match last {
        Some(AgentEvent::Stopped) => "cancelled",
        Some(AgentEvent::StepLimit) => "max_turn_requests",
        _ => "end_turn",
    }
}

/// The choices a client is offered: once, or not.
pub(super) fn permission_options() -> Value {
    json!([
        { "optionId": "allow_once", "name": "Allow once", "kind": "allow_once" },
        { "optionId": "reject_once", "name": "Reject", "kind": "reject_once" },
    ])
}

/// Only an explicit choice of the allow option allows: a cancel, an error or anything unexpected is a no (R4).
pub(super) fn allowed(response: &Value) -> bool {
    let outcome = &response["result"]["outcome"];
    outcome["outcome"] == "selected" && outcome["optionId"] == "allow_once"
}

/// The words of a prompt. Links are named; anything else is refused rather than silently dropped.
pub(super) fn prompt_text(blocks: &Value) -> Result<String, String> {
    let blocks = blocks
        .as_array()
        .ok_or("`prompt` must be an array of content blocks")?;
    let mut parts = Vec::new();
    for block in blocks {
        match block["type"].as_str() {
            Some("text") => parts.push(
                block["text"]
                    .as_str()
                    .ok_or("a text block needs `text`")?
                    .to_owned(),
            ),
            Some("resource_link") => {
                let uri = block["uri"].as_str().ok_or("a resource link needs `uri`")?;
                let name = block["name"].as_str().unwrap_or(uri);
                parts.push(format!("[{name}]({uri})"));
            }
            other => {
                return Err(format!(
                    "unsupported content block: {}",
                    other.unwrap_or("untyped")
                ));
            }
        }
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        return Err("the prompt is empty".to_owned());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_choosing_to_allow_allows() {
        let said = |outcome: Value| allowed(&json!({ "result": { "outcome": outcome } }));
        assert!(said(
            json!({ "outcome": "selected", "optionId": "allow_once" })
        ));
        assert!(!said(
            json!({ "outcome": "selected", "optionId": "reject_once" })
        ));
        assert!(!said(
            json!({ "outcome": "selected", "optionId": "allow_always" })
        ));
        assert!(!said(json!({ "outcome": "cancelled" })));
        assert!(!allowed(
            &json!({ "error": { "code": -32601, "message": "no" } })
        ));
        assert!(!allowed(&json!({})));
    }

    #[test]
    fn a_prompt_is_its_text_and_links_and_nothing_else_is_swallowed() {
        let ok = json!([{ "type": "text", "text": "see" }, { "type": "resource_link", "uri": "file:///a", "name": "a" }]);
        assert_eq!(prompt_text(&ok).unwrap(), "see\n[a](file:///a)");
        assert!(prompt_text(&json!([{ "type": "image", "data": "x" }])).is_err());
        assert!(prompt_text(&json!([])).is_err());
        assert!(prompt_text(&json!("hi")).is_err());
    }
}
