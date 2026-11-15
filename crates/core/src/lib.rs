//! Domain types shared by every crate: threads, messages, tool calls, ids.
//! No I/O and no async here; this crate stays dependency-light on purpose.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable identifier of a conversation thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

/// A request from the model to run one tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    /// Set on assistant messages that ask for tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set on tool messages: which call this answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    #[must_use]
    pub fn new(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    #[must_use]
    pub fn assistant(text: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Self::new(Role::Assistant, text)
        }
    }

    #[must_use]
    pub fn tool_result(call_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::new(Role::Tool, text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_message_round_trips_through_json() {
        let m = Message::new(Role::Assistant, "hi");
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"role":"assistant","text":"hi"}"#);
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), m);
    }

    #[test]
    fn tool_result_carries_the_call_id() {
        let m = Message::tool_result("c1", "ok");
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.tool_call_id.as_deref(), Some("c1"));
    }

    #[test]
    fn thread_id_serializes_as_plain_string() {
        let id = ThreadId("t1".into());
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""t1""#);
    }
}
