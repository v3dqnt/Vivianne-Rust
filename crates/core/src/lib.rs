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

/// How much the model should think before answering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Thinking {
    Off,
    Low,
    #[default]
    Medium,
    High,
}

impl Thinking {
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    /// The level's name as providers spell reasoning effort (`low`, `medium`, `high`);
    /// `None` when thinking is off.
    #[must_use]
    pub fn effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
        }
    }
}

/// What the agent is for right now. Each mode adds a short instruction to the system prompt and limits which
/// tools the agent may use, so the same agent visibly behaves differently depending on what the user wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Reads, writes, runs commands: everything.
    #[default]
    Agent,
    /// Answers questions about the project. Looks, never changes or runs anything.
    Ask,
    /// Finds the cause of a problem: looks and runs commands, but writes no files.
    Debug,
    /// Gets oriented in the codebase. Looks only.
    Explore,
}

/// Tools that only look. Anything not named here (a new tool, an editing one such as `lsp_edit`) is withheld
/// outside `Agent` mode until someone decides it is safe: a list of what is allowed fails closed, a list of what
/// is forbidden fails open.
const LOOK_ONLY_TOOLS: [&str; 8] = ["read", "ls", "grep", "code_search", "glob", "diagnostics", "lsp", "repo_overview"];
/// What `Debug` adds to looking.
const RUN_TOOLS: [&str; 1] = ["shell"];

impl Mode {
    pub const ALL: [Self; 4] = [Self::Agent, Self::Ask, Self::Debug, Self::Explore];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Ask => "Ask",
            Self::Debug => "Debug",
            Self::Explore => "Explore",
        }
    }

    /// One line for the menu.
    #[must_use]
    pub fn hint(self) -> &'static str {
        match self {
            Self::Agent => "Reads, writes and runs commands",
            Self::Ask => "Answers questions, changes nothing",
            Self::Debug => "Finds a cause: runs commands, writes no files",
            Self::Explore => "Maps the codebase, looks only",
        }
    }

    /// The instruction added to the system prompt, or `None` for the unrestricted default.
    #[must_use]
    pub fn prompt_section(self) -> Option<&'static str> {
        match self {
            Self::Agent => None,
            Self::Ask => Some(
                "# Mode: Ask
You are in Ask mode: answer questions about the codebase and project. Read and search as much as you need,                  but do not write, edit or run commands; you have no such tools right now. If the request really needs a change, say so                  and suggest switching to Agent mode instead of working around it.",
            ),
            Self::Debug => Some(
                "# Mode: Debug
You are in Debug mode: find the root cause of a problem. You can read, search and run commands                  (reproduce the bug, run tests, inspect logs), but you cannot write or edit files: this mode is for diagnosis, not                  fixing. End with a clear statement of the cause and what change would fix it, so the user can act on it or switch to                  Agent mode.",
            ),
            Self::Explore => Some(
                "# Mode: Explore
You are in Explore mode: help the user get oriented in the codebase. Read, search and describe                  structure and architecture; you have only read-only navigation tools. Favor a clear map of how things fit together                  over deep investigation of any one issue.",
            ),
        }
    }

    /// Whether the agent may use the tool called `name` in this mode.
    #[must_use]
    pub fn allows(self, name: &str) -> bool {
        match self {
            Self::Agent => true,
            Self::Ask | Self::Explore => LOOK_ONLY_TOOLS.contains(&name),
            Self::Debug => LOOK_ONLY_TOOLS.contains(&name) || RUN_TOOLS.contains(&name),
        }
    }
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
    fn thinking_names_its_effort_and_defaults_to_medium() {
        assert_eq!(Thinking::default(), Thinking::Medium);
        let efforts: Vec<_> = Thinking::ALL.iter().map(|t| t.effort()).collect();
        assert_eq!(efforts, [None, Some("low"), Some("medium"), Some("high")]);
    }

    #[test]
    fn modes_allow_only_what_they_are_for_and_unknown_tools_are_withheld() {
        assert!(Mode::Agent.allows("anything"));
        for tool in ["write", "edit", "lsp_edit", "repo_clone", "shell", "a_tool_added_next_year"] {
            assert!(!Mode::Ask.allows(tool) && !Mode::Explore.allows(tool), "{tool}");
        }
        assert!(Mode::Ask.allows("read") && Mode::Explore.allows("grep"));
        assert!(Mode::Debug.allows("shell") && Mode::Debug.allows("read"));
        assert!(!Mode::Debug.allows("write") && !Mode::Debug.allows("edit") && !Mode::Debug.allows("lsp_edit"));
        assert!(Mode::Agent.prompt_section().is_none());
        assert!(Mode::ALL.iter().skip(1).all(|m| m.prompt_section().is_some()));
    }

    #[test]
    fn thread_id_serializes_as_plain_string() {
        let id = ThreadId("t1".into());
        assert_eq!(serde_json::to_string(&id).unwrap(), r#""t1""#);
    }
}
